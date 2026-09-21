//! Emulator stream ownership and lifecycle.
//!
//! Platform adapters discover and drive devices; this module owns only the
//! process/session facts shared by both adapters: start de-duplication,
//! cancellation, pause/resume, payload backpressure and child collection.

use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use super::{BinaryChannel, EmulatorPlatform, EmulatorStream};

const START_LEASE_TIMEOUT: Duration = Duration::from_secs(210);
const PAYLOAD_ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// How many payloads may be waiting on the webview at once.
///
/// Two, and not more, because this is a mirror somebody is TOUCHING. Every
/// extra slot is another whole frame of distance between the finger and the
/// picture — at the 16ms ceiling a third slot buys no throughput the second
/// one has not already bought and costs another frame of lag. Two is the
/// smallest number that lets the two IPC directions overlap: frame k+1 is
/// being pulled while frame k's acknowledgement is still on its way back.
const PAYLOAD_WINDOW: usize = 2;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum StreamMode {
    Frames,
    Video,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct SessionKey {
    pub platform: EmulatorPlatform,
    pub device: String,
    pub mode: StreamMode,
}

impl SessionKey {
    pub fn frames(platform: EmulatorPlatform, device: impl Into<String>) -> Self {
        Self {
            platform,
            device: device.into(),
            mode: StreamMode::Frames,
        }
    }

    pub fn video(platform: EmulatorPlatform, device: impl Into<String>) -> Self {
        Self {
            platform,
            device: device.into(),
            mode: StreamMode::Video,
        }
    }
}

/// Mutable runtime controls for one stream.
///
/// A single condition variable owns every reason the pump may wake: input,
/// payload acknowledgement, resume and shutdown. That keeps the hot path from
/// polling several atomics on separate timers.
pub(super) struct SessionControl {
    alive: AtomicBool,
    paused: AtomicBool,
    /// Whether somebody is attending to the pane — pointer over the screen or
    /// keyboard focus in it. A pane merely in view is a mirror glanced at
    /// while the terminal is read, and is pushed at a fraction of the rate.
    engaged: AtomicBool,
    /// When the device was last given an input — a person's touch or an
    /// agent's `zerocode-emulator tap`. The moments after one are when the
    /// screen moves in a way somebody wants to see, whoever is attending.
    last_input: Mutex<Option<Instant>>,
    next_sequence: AtomicU64,
    /// The payloads posted and not yet acknowledged, oldest first, each with
    /// the moment it was posted.
    posted: Mutex<VecDeque<(u64, Instant)>>,
    signal: Mutex<bool>,
    wake: Condvar,
    child: Mutex<Option<Child>>,
    /// A marked press holds this from its fresh tree through input so another
    /// input in this window cannot slip between the proof and the tap. The
    /// registry shares it by actual device, across stream modes and restarts.
    input: Arc<Mutex<()>>,
    /// The longest edge this stream's viewer can actually paint, in the device
    /// pixels it will paint with. Zero means nobody has said.
    ///
    /// Nothing here decides what that means — the platform pump reads it and
    /// asks its own source for a picture that size. It lives on the session
    /// because the pane that resizes and the pump that encodes only ever meet
    /// through this handle, and an atomic rather than a lock because the pump
    /// reads it on every frame while a resize writes it a few times a second.
    viewport_long_edge: AtomicU32,
    cleanup: Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>,
    /// Whether the pump last found the DEVICE on the bridge.
    ///
    /// The pump reads `adb` once a turn and leaves the answer here; the input
    /// door reads it for nothing. It is a HINT, not the verdict: it is only
    /// ever believed far enough to make an input road ask `adb` itself before
    /// refusing, because a pane parked in the background stops reading and a
    /// memo frozen on "gone" must never turn away a device that came back.
    /// True until a pump says otherwise — a session with no pump refuses
    /// nothing on this account.
    on_the_bridge: AtomicBool,
    /// The door this stream's pictures go through.
    ///
    /// A slot the session owns rather than an argument the pump carries,
    /// because the door can CHANGE under a living pump: a pane opening a
    /// device that already has a session is handed that session
    /// (`StartClaim::Existing`) and arrives with a channel of its own, while
    /// the channel the first pane opened has usually just been disposed on
    /// its way in. A pump holding the old one posts every frame into a closed
    /// door — measured: `channel.send` fails, the pump calls that the stream
    /// ending and breaks, and because that break writes no log line the pane
    /// goes quiet with nothing anywhere saying why.
    frames: Mutex<Option<BinaryChannel>>,
}

impl SessionControl {
    fn with_input(input: Arc<Mutex<()>>) -> Arc<Self> {
        Arc::new(Self {
            alive: AtomicBool::new(true),
            paused: AtomicBool::new(false),
            engaged: AtomicBool::new(true),
            last_input: Mutex::new(None),
            // Zero is the sequence `acknowledge` ignores, so the wire starts at 1.
            next_sequence: AtomicU64::new(1),
            posted: Mutex::new(VecDeque::new()),
            signal: Mutex::new(false),
            wake: Condvar::new(),
            child: Mutex::new(None),
            input,
            viewport_long_edge: AtomicU32::new(0),
            cleanup: Mutex::new(None),
            on_the_bridge: AtomicBool::new(true),
            frames: Mutex::new(None),
        })
    }

    #[cfg(test)]
    fn new() -> Arc<Self> {
        Self::with_input(Arc::default())
    }

    pub fn set_cleanup(&self, cleanup: impl FnOnce() + Send + 'static) {
        *held(&self.cleanup) = Some(Box::new(cleanup));
    }

    pub fn input(&self) -> Result<SessionInput<'_>, String> {
        let guard = held(&self.input);
        if !self.is_alive() {
            return Err("이 기기 스트림은 실행 중이 아닙니다".into());
        }
        Ok(SessionInput {
            control: self,
            _guard: guard,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn is_engaged(&self) -> bool {
        self.engaged.load(Ordering::Acquire)
    }

    /// What the pump last saw of the device.
    pub fn was_on_the_bridge(&self) -> bool {
        self.on_the_bridge.load(Ordering::Acquire)
    }

    /// The pump's reading of the device, published for the input door.
    pub fn note_on_the_bridge(&self, present: bool) {
        self.on_the_bridge.store(present, Ordering::Release);
    }

    /// Engaged until the window says otherwise: a pane that never reports
    /// keeps the full rate rather than starving.
    pub fn set_engaged(&self, engaged: bool) {
        self.engaged.store(engaged, Ordering::Release);
        self.notify();
    }

    /// The device was just given an input.
    pub fn note_input(&self) {
        *held(&self.last_input) = Some(Instant::now());
    }

    /// Was the device given an input within `window` of now?
    pub fn input_within(&self, window: Duration) -> bool {
        held(&self.last_input).is_some_and(|at| at.elapsed() < window)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
        if paused {
            // A video reader can be blocked on a still screen. Killing the
            // producer is the only prompt way to pause it; the pump respawns
            // a fresh producer (and fresh codec headers) on resume.
            self.kill_child();
            // And the payloads in flight are let go of, exactly as `stop`
            // lets go of them. A pane is paused BECAUSE it stopped rendering,
            // so the frames it was sent will never be acknowledged — and a
            // reservation nobody will ever release costs the first frame
            // after the resume the whole PAYLOAD_ACK_TIMEOUT before
            // `wait_for_payload_slot` force-drops it. Three seconds of the
            // stale last picture, every time a pane comes back.
            held(&self.posted).clear();
        }
        self.notify();
    }

    pub fn wait_until_running(&self) -> bool {
        let mut notified = held(&self.signal);
        while self.is_alive() && self.is_paused() {
            notified = self
                .wake
                .wait(notified)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        self.is_alive()
    }

    pub fn rest(&self, duration: Duration) -> bool {
        if !self.is_alive() {
            return false;
        }
        let mut notified = held(&self.signal);
        if !*notified {
            let (next, _) = self
                .wake
                .wait_timeout(notified, duration)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            notified = next;
        }
        *notified = false;
        self.is_alive()
    }

    pub fn notify(&self) {
        *held(&self.signal) = true;
        self.wake.notify_all();
    }

    /// Wait until the renderer has room for another payload.
    ///
    /// The window is what the acknowledgement MEANS. At one, an ack is a
    /// latency gate: the pump may not take the next picture until the webview
    /// has answered for the last one, and the pump waits here at the TOP of
    /// its loop (ios.rs, android.rs) before it starts the clock it measures
    /// FRAME_CEILING against — so that answer's cost lands on top of the
    /// ceiling rather than inside it. At two the ack is what it should be, a
    /// backpressure signal, and the two IPC directions overlap.
    ///
    /// Losing an ack must still not retain the stream, so each posted payload
    /// carries the moment it was posted and is dropped once it is past the
    /// deadline — a per-payload clock rather than one global flag, because
    /// with more than one outstanding there is no single "the" reservation.
    pub fn wait_for_payload_slot(&self) -> bool {
        loop {
            if !self.is_alive() {
                return false;
            }
            let wait = {
                let mut posted = held(&self.posted);
                while posted
                    .front()
                    .is_some_and(|(_, at)| at.elapsed() >= PAYLOAD_ACK_TIMEOUT)
                {
                    posted.pop_front();
                }
                if posted.len() < PAYLOAD_WINDOW {
                    return self.is_alive() && !self.is_paused();
                }
                posted.front().map_or(Duration::ZERO, |(_, at)| {
                    PAYLOAD_ACK_TIMEOUT.saturating_sub(at.elapsed())
                })
            };
            self.rest(wait.min(Duration::from_millis(50)));
        }
    }

    pub fn reserve_payload(&self) -> Option<u64> {
        if !self.wait_for_payload_slot() {
            return None;
        }
        let sequence = self.mint_sequence();
        held(&self.posted).push_back((sequence, Instant::now()));
        Some(sequence)
    }

    /// Take a slot if one is free this instant, and refuse rather than wait.
    ///
    /// What a pushed frame wants, where `reserve_payload` is what a pulled one
    /// wants. A pump that ASKS for each picture has nothing better to do than
    /// wait for the renderer, because the picture it would take instead does
    /// not exist yet and taking it is the expensive part. A pump being pushed
    /// to is the other way round: the next picture is already on its way, so a
    /// frame that cannot go now is better dropped than queued — queuing it only
    /// guarantees the person sees a picture of the past.
    ///
    /// The window is checked and the slot taken under one lock, because unlike
    /// the waiting road this one is allowed to be raced by the acknowledgement
    /// coming back on another thread.
    pub fn try_reserve_payload(&self) -> Option<u64> {
        if !self.is_alive() || self.is_paused() {
            return None;
        }
        let mut posted = held(&self.posted);
        while posted
            .front()
            .is_some_and(|(_, at)| at.elapsed() >= PAYLOAD_ACK_TIMEOUT)
        {
            posted.pop_front();
        }
        if posted.len() >= PAYLOAD_WINDOW {
            return None;
        }
        let sequence = self.mint_sequence();
        posted.push_back((sequence, Instant::now()));
        Some(sequence)
    }

    fn mint_sequence(&self) -> u64 {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        if sequence == 0 {
            // An overflow after centuries of frames must not mint the sentinel.
            return self.next_sequence.fetch_add(1, Ordering::Relaxed);
        }
        sequence
    }

    /// Retire an acknowledged payload and every payload older than it.
    ///
    /// Older ones go with it because the webview takes them in the order they
    /// were posted — tauri's Channel delivers `onmessage` strictly in
    /// sequence — so an ack for a later one is proof the earlier ones
    /// arrived. And an earlier one can genuinely go unanswered: a payload that
    /// lands before the pane has bound its door waits in a single `pending`
    /// slot (`makeEmulatorBinaryDoor` in ui/shell.js), so a second one
    /// arriving first overwrites it and nobody ever acknowledges the first.
    /// Without this rule that one lost ack would hold its slot for the whole
    /// PAYLOAD_ACK_TIMEOUT. Do not simplify it back to an exact-match removal.
    pub fn acknowledge(&self, sequence: u64) {
        if sequence == 0 {
            return;
        }
        {
            let mut posted = held(&self.posted);
            if let Some(at) = posted.iter().position(|(one, _)| *one == sequence) {
                posted.drain(..=at);
            }
        }
        self.notify();
    }

    /// Point this stream's pictures at a new door.
    ///
    /// The reservations go with it: the payloads in flight were posted
    /// through the door being replaced, so the pane that would have
    /// acknowledged them is gone and holding their slots would cost the new
    /// door the whole PAYLOAD_ACK_TIMEOUT before its first picture.
    pub fn hand_frames_to(&self, channel: BinaryChannel) {
        *held(&self.frames) = Some(channel);
        held(&self.posted).clear();
        self.notify();
    }

    /// The door as it stands this frame. Cloned rather than borrowed so the
    /// send happens outside the lock — a pump blocking the swap would be the
    /// same wedge from the other side.
    pub fn frame_door(&self) -> Option<BinaryChannel> {
        held(&self.frames).clone()
    }

    pub fn install_child(&self, mut child: Child) -> bool {
        if !self.is_alive() || self.is_paused() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        let mut slot = held(&self.child);
        if let Some(mut previous) = slot.take() {
            let _ = previous.kill();
            let _ = previous.wait();
        }
        *slot = Some(child);
        true
    }

    pub fn child_status(&self) -> Option<std::io::Result<Option<std::process::ExitStatus>>> {
        held(&self.child).as_mut().map(Child::try_wait)
    }

    pub fn take_child(&self) -> Option<Child> {
        held(&self.child).take()
    }

    pub fn kill_child(&self) {
        let Some(mut child) = self.take_child() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }

    pub fn viewport_long_edge(&self) -> Option<u32> {
        match self.viewport_long_edge.load(Ordering::Acquire) {
            0 => None,
            asked => Some(asked),
        }
    }

    /// Tell this stream how big a picture its viewer can show.
    ///
    /// The pump is woken rather than left to find out on its next turn: a pane
    /// that was just resized is a pane somebody is looking at, and the whole
    /// point of the number is that the next picture already honours it.
    pub fn set_viewport_long_edge(&self, long_edge: u32) {
        self.viewport_long_edge.store(long_edge, Ordering::Release);
        self.notify();
    }

    fn cancel(&self) -> bool {
        self.alive.swap(false, Ordering::AcqRel)
    }

    #[cfg(test)]
    fn stop(&self) {
        if self.cancel() {
            self.finish_stop();
        }
    }

    fn finish_stop(&self) {
        held(&self.posted).clear();
        self.kill_child();
        self.notify();
        if let Some(cleanup) = held(&self.cleanup).take() {
            cleanup();
        }
    }
}

/// One device's input gate, retaining the exact stream that acquired it.
/// Stopping a stream revokes it without waiting for a slow tree read; a new
/// stream still shares the gate until every in-flight input has left it.
pub(super) struct SessionInput<'control> {
    control: &'control SessionControl,
    _guard: MutexGuard<'control, ()>,
}

impl SessionInput<'_> {
    pub fn is_alive(&self) -> bool {
        self.control.is_alive()
    }
}

struct SessionEntry {
    key: SessionKey,
    descriptor: EmulatorStream,
    control: Arc<SessionControl>,
}

#[derive(Default)]
struct RegistryState {
    sessions: HashMap<String, SessionEntry>,
    active: HashMap<SessionKey, String>,
    starting: HashSet<SessionKey>,
    inputs: HashMap<(EmulatorPlatform, String), Weak<Mutex<()>>>,
}

#[derive(Default)]
pub(super) struct SessionRegistry {
    state: Mutex<RegistryState>,
    changed: Condvar,
}

pub(super) enum StartClaim {
    Existing(EmulatorStream),
    Acquired(StartLease),
}

pub(super) struct StartLease {
    registry: &'static SessionRegistry,
    key: Option<SessionKey>,
}

impl StartLease {
    pub fn activate(
        mut self,
        descriptor: EmulatorStream,
        control: Arc<SessionControl>,
    ) -> EmulatorStream {
        let key = self.key.take().expect("start lease already consumed");
        let mut state = held(&self.registry.state);
        state.starting.remove(&key);
        state.active.insert(key.clone(), descriptor.stream.clone());
        state.sessions.insert(
            descriptor.stream.clone(),
            SessionEntry {
                key,
                descriptor: descriptor.clone(),
                control,
            },
        );
        self.registry.changed.notify_all();
        descriptor
    }
}

impl Drop for StartLease {
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        held(&self.registry.state).starting.remove(&key);
        self.registry.changed.notify_all();
    }
}

impl SessionRegistry {
    pub fn new_control(&self, descriptor: &EmulatorStream) -> Arc<SessionControl> {
        let mut state = held(&self.state);
        // A stopped stream may still own a pending Pin. Its strong handle
        // keeps the device gate alive for any replacement stream. Dead weak
        // entries are pruned rather than retaining every past device forever.
        state.inputs.retain(|_, gate| gate.strong_count() > 0);
        let gate = state
            .inputs
            .entry((descriptor.platform, descriptor.udid.clone()))
            .or_default();
        let input = gate.upgrade().unwrap_or_else(|| {
            let input = Arc::default();
            *gate = Arc::downgrade(&input);
            input
        });
        SessionControl::with_input(input)
    }

    pub fn claim(&'static self, key: SessionKey) -> Result<StartClaim, String> {
        let deadline = Instant::now() + START_LEASE_TIMEOUT;
        let mut state = held(&self.state);
        loop {
            if let Some(stream) = state.active.get(&key)
                && let Some(entry) = state.sessions.get(stream)
                && entry.control.is_alive()
            {
                let mut existing = entry.descriptor.clone();
                existing.reused = true;
                return Ok(StartClaim::Existing(existing));
            }
            if !state.starting.contains(&key) {
                state.starting.insert(key.clone());
                return Ok(StartClaim::Acquired(StartLease {
                    registry: self,
                    key: Some(key),
                }));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err("에뮬레이터 시작 작업이 완료되지 않았습니다".to_string());
            }
            let (next, timed) = self
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if timed.timed_out() {
                return Err("에뮬레이터 시작 작업이 시간을 초과했습니다".to_string());
            }
        }
    }

    pub fn control(&self, stream: &str) -> Option<Arc<SessionControl>> {
        held(&self.state)
            .sessions
            .get(stream)
            .map(|entry| entry.control.clone())
    }

    pub fn target_control(
        &self,
        platform: EmulatorPlatform,
        target: &str,
    ) -> Option<Arc<SessionControl>> {
        held(&self.state)
            .sessions
            .values()
            .find(|entry| {
                entry.descriptor.platform == platform
                    && entry.descriptor.udid == target
                    && entry.control.is_alive()
            })
            .map(|entry| entry.control.clone())
    }

    /// How many live panes this platform has open right now.
    ///
    /// The idle reclaimer's whole question (D3): a fleet nobody is looking at
    /// is a fleet that can be put away, and a fleet with one pane on it is
    /// not, whichever device that pane is on.
    pub fn live_count(&self, platform: EmulatorPlatform) -> usize {
        held(&self.state)
            .sessions
            .values()
            .filter(|entry| entry.descriptor.platform == platform && entry.control.is_alive())
            .count()
    }

    pub fn stop(&self, stream: &str) -> bool {
        let (entry, first) = {
            let mut state = held(&self.state);
            let Some(entry) = state.sessions.remove(stream) else {
                return false;
            };
            // Revoke before another claim can publish a replacement. Cleanup
            // remains outside this short registry lock and never waits on the
            // device input gate held during a potentially slow AX read.
            let first = entry.control.cancel();
            if state
                .active
                .get(&entry.key)
                .is_some_and(|active| active == stream)
            {
                state.active.remove(&entry.key);
            }
            (entry, first)
        };
        if first {
            entry.control.finish_stop();
        }
        self.changed.notify_all();
        true
    }

    pub fn finish(&self, stream: &str) {
        let _ = self.stop(stream);
    }

    /// Point a stream's pictures at the door its newest caller brought.
    ///
    /// One device has one screen and therefore one pump, so a pane opening a
    /// device somebody already has is handed that session rather than a second
    /// pump. What that handing over must not skip is the CHANNEL: the caller
    /// arrives with a fresh one and the pump is still holding the first
    /// caller's, which the first caller usually closed on its way in.
    pub fn hand_frames_to(&self, stream: &str, channel: BinaryChannel) {
        if let Some(control) = self.control(stream) {
            control.hand_frames_to(channel);
        }
    }

    pub fn acknowledge(&self, stream: &str, sequence: u64) {
        if let Some(control) = self.control(stream) {
            control.acknowledge(sequence);
        }
    }

    pub fn set_paused(&self, stream: &str, paused: bool) {
        if let Some(control) = self.control(stream) {
            control.set_paused(paused);
        }
    }

    pub fn set_viewport(&self, stream: &str, long_edge: u32) {
        if let Some(control) = self.control(stream) {
            control.set_viewport_long_edge(long_edge);
        }
    }

    pub fn set_engaged(&self, stream: &str, engaged: bool) {
        if let Some(control) = self.control(stream) {
            control.set_engaged(engaged);
        }
    }

    /// The device was poked: the pump looks again, and knows it was poked —
    /// the seconds after an input are the ones the pane is pushed at full
    /// rate for, whether or not anybody's pointer is over it.
    pub fn nudge(&self, platform: EmulatorPlatform, target: &str) {
        if let Some(control) = self.target_control(platform, target) {
            control.note_input();
            control.notify();
        }
    }

    pub fn shutdown_all(&self) {
        let entries = {
            let mut state = held(&self.state);
            state.active.clear();
            state.starting.clear();
            state
                .sessions
                .drain()
                .map(|(_, entry)| entry)
                .filter(|entry| entry.control.cancel())
                .collect::<Vec<_>>()
        };
        for entry in entries {
            entry.control.finish_stop();
        }
        self.changed.notify_all();
    }

    #[cfg(test)]
    fn counts(&self) -> (usize, usize, usize) {
        let state = held(&self.state);
        (
            state.sessions.len(),
            state.active.len(),
            state.starting.len(),
        )
    }
}

pub(super) fn registry() -> &'static SessionRegistry {
    static REGISTRY: OnceLock<SessionRegistry> = OnceLock::new();
    REGISTRY.get_or_init(SessionRegistry::default)
}

pub(super) struct FinishSession {
    stream: String,
}

impl FinishSession {
    pub fn new(stream: impl Into<String>) -> Self {
        Self {
            stream: stream.into(),
        }
    }
}

impl Drop for FinishSession {
    fn drop(&mut self) {
        registry().finish(&self.stream);
    }
}

fn held<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(stream: &str) -> EmulatorStream {
        EmulatorStream {
            stream: stream.to_string(),
            udid: "device".to_string(),
            name: "Device".to_string(),
            platform: EmulatorPlatform::Android,
            interactive: true,
            reused: false,
        }
    }

    #[test]
    fn a_start_claim_reuses_one_live_session_and_stop_clears_every_index() {
        let registry = Box::leak(Box::<SessionRegistry>::default());
        let key = SessionKey::frames(EmulatorPlatform::Android, "device");
        let StartClaim::Acquired(lease) = registry.claim(key.clone()).expect("first claim") else {
            panic!("first claim unexpectedly reused a session");
        };
        lease.activate(descriptor("stream-1"), SessionControl::new());

        let StartClaim::Existing(reused) = registry.claim(key).expect("second claim") else {
            panic!("second claim did not reuse the live session");
        };
        assert_eq!(reused.stream, "stream-1");
        assert!(reused.reused);
        assert_eq!(registry.counts(), (1, 1, 0));

        assert!(registry.stop("stream-1"));
        assert_eq!(registry.counts(), (0, 0, 0));
    }

    fn registered_control(
        registry: &'static SessionRegistry,
        key: SessionKey,
        stream: &str,
    ) -> Arc<SessionControl> {
        let StartClaim::Acquired(lease) = registry.claim(key).expect("new stream") else {
            panic!("unexpected existing stream");
        };
        let descriptor = descriptor(stream);
        let control = registry.new_control(&descriptor);
        lease.activate(descriptor, control.clone());
        control
    }

    #[test]
    fn device_input_excludes_another_stream_mode_and_a_reopened_stream() {
        for reopen in [false, true] {
            let registry = Box::leak(Box::<SessionRegistry>::default());
            // Android frame claims use an AVD name; video claims use its
            // serial. The descriptors name the same actual device in both.
            let frames = SessionKey::frames(EmulatorPlatform::Android, "avd");
            let old = registered_control(registry, frames.clone(), "old");
            let _snapshot_to_tap = held(&old.input);
            let key = if reopen {
                assert!(registry.stop("old"));
                frames
            } else {
                SessionKey::video(EmulatorPlatform::Android, "device")
            };
            let new = registered_control(registry, key, "new");
            assert!(
                matches!(
                    new.input.try_lock(),
                    Err(std::sync::TryLockError::WouldBlock)
                ),
                "new input can cross an in-flight Pin proof (reopen={reopen})"
            );
        }
    }

    #[test]
    fn stopping_revokes_a_held_input_without_waiting_for_the_device_gate() {
        let registry = Box::leak(Box::<SessionRegistry>::default());
        let key = SessionKey::frames(EmulatorPlatform::Android, "avd");
        let old = registered_control(registry, key.clone(), "old");
        let input = old.input().expect("live input");
        assert!(registry.stop("old"));
        assert!(!input.is_alive());
        let new = registered_control(registry, key, "new");
        assert!(matches!(
            new.input.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        drop(input);
        assert!(old.input().is_err(), "a revoked stream regained input");
        assert!(new.input().is_ok());
    }

    #[test]
    fn input_gates_are_per_actual_device_and_dead_entries_are_reclaimed() {
        let registry = SessionRegistry::default();
        let first = registry.new_control(&descriptor("first"));
        let _input = first.input().expect("first device");
        let mut other = descriptor("other");
        other.udid.push_str("-other");
        let second = registry.new_control(&other);
        assert!(second.input().is_ok(), "different devices share a gate");
        drop(second);
        let next = registry.new_control(&descriptor("next"));
        assert_eq!(held(&registry.state).inputs.len(), 1);
        assert!(Arc::ptr_eq(&first.input, &next.input));
    }

    #[test]
    fn an_ack_retires_its_payload_and_every_older_one_but_never_a_newer_one() {
        let control = SessionControl::new();
        let first = control.reserve_payload().expect("first slot");
        let second = control.reserve_payload().expect("second slot");
        assert_eq!(held(&control.posted).len(), PAYLOAD_WINDOW);
        // An ack for a sequence nobody posted moves nothing.
        control.acknowledge(second + 1);
        assert_eq!(held(&control.posted).len(), PAYLOAD_WINDOW);
        // The older one alone leaves the newer one holding its slot.
        control.acknowledge(first);
        assert_eq!(held(&control.posted).len(), 1);
        control.acknowledge(second);
        assert!(held(&control.posted).is_empty());
    }

    #[test]
    fn a_pushed_frame_is_refused_a_slot_rather_than_made_to_wait_for_one() {
        let control = SessionControl::new();
        let first = control.try_reserve_payload().expect("first slot");
        let second = control.try_reserve_payload().expect("second slot");
        // The window is full, and the pushed road hears that immediately
        // instead of blocking until an ack or the three-second timeout.
        assert!(control.try_reserve_payload().is_none());
        control.acknowledge(first);
        let third = control
            .try_reserve_payload()
            .expect("slot freed by the ack");
        assert!(third > second);
        assert!(control.try_reserve_payload().is_none());
    }

    #[test]
    fn a_paused_or_stopped_stream_hands_out_no_pushed_slots() {
        let control = SessionControl::new();
        control.set_paused(true);
        assert!(control.try_reserve_payload().is_none());
        control.set_paused(false);
        assert!(control.try_reserve_payload().is_some());
        control.stop();
        assert!(control.try_reserve_payload().is_none());
    }

    #[test]
    fn a_viewport_is_unset_until_a_pane_says_and_reads_back_what_it_said() {
        let control = SessionControl::new();
        assert_eq!(control.viewport_long_edge(), None);
        control.set_viewport_long_edge(1280);
        assert_eq!(control.viewport_long_edge(), Some(1280));
        // Zero is how "nobody has said" is spelled on the wire, not a size.
        control.set_viewport_long_edge(0);
        assert_eq!(control.viewport_long_edge(), None);
    }

    /// A fresh control is engaged — nobody has said otherwise — and the
    /// window's word flips it both ways.
    #[test]
    fn a_control_is_engaged_until_the_window_says_otherwise() {
        let control = SessionControl::new();
        assert!(control.is_engaged());
        control.set_engaged(false);
        assert!(!control.is_engaged());
        control.set_engaged(true);
        assert!(control.is_engaged());
    }

    /// An input is remembered for as long as the caller's window says, and
    /// a control that was never poked remembers nothing.
    #[test]
    fn an_input_is_remembered_for_a_window() {
        let control = SessionControl::new();
        assert!(!control.input_within(Duration::from_secs(60)));
        control.note_input();
        assert!(control.input_within(Duration::from_secs(60)));
        assert!(!control.input_within(Duration::ZERO));
    }

    #[test]
    fn pause_and_stop_wake_a_waiting_pump() {
        let control = SessionControl::new();
        control.set_paused(true);
        assert!(control.is_paused());
        control.set_paused(false);
        assert!(control.wait_until_running());
        control.stop();
        assert!(!control.wait_until_running());
    }
}
