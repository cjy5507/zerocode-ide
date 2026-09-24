import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOS
@testable import ZeroCodeComputerUseMacOSCore

// MARK: - Fakes: a clock the test moves, sleepers it scripts, a poster that records

/// A host clock the test moves; it never goes back.
final class FakeClock: HandClock, @unchecked Sendable {
    private let lock = NSLock()
    private var now: UInt64

    init(_ start: UInt64 = 1_000_000_000) { now = start }

    func nowNs() -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return now
    }

    func set(_ value: UInt64) {
        lock.lock()
        now = max(now, value)
        lock.unlock()
    }
}

/// A sleeper in virtual time: every sleep runs the test's step for it, then
/// the clock jumps to the deadline — no real time passes.
final class ScriptedSleeper: HandSleeper, @unchecked Sendable {
    private let lock = NSLock()
    private let clock: FakeClock
    private var count = 0
    var onSleep: ((Int, UInt64) -> Void)?

    init(clock: FakeClock) { self.clock = clock }

    func sleep(untilNs deadlineNs: UInt64) {
        lock.lock()
        let index = count
        count += 1
        let step = onSleep
        lock.unlock()
        step?(index, deadlineNs)
        clock.set(deadlineNs)
    }

    func wake() {}
}

/// A sleeper nothing wakes but a wake: a hold with no end of its own. Real
/// time bounds it, so a test that fails does not hang the suite.
final class GateSleeper: HandSleeper, @unchecked Sendable {
    private let gate = DispatchSemaphore(value: 0)

    func sleep(untilNs deadlineNs: UInt64) {
        _ = gate.wait(timeout: .now() + 10)
    }

    func wake() { gate.signal() }
}

/// Records what the hand posts and keeps the pointer where the hand put it,
/// as the window server does.
final class RecordingPoster: HandPoster, @unchecked Sendable {
    struct Posted: Equatable {
        let event: HandEvent
        let tag: Int64
    }

    private let lock = NSLock()
    private var posted: [Posted] = []
    private var location = SmoothPointerPath.Point(x: 0, y: 0)
    /// Runs after each post, outside the poster's lock (but inside the hand's:
    /// it must not call the hand).
    var onPost: ((HandEvent, Int64) -> Void)?

    func post(_ event: HandEvent, tag: Int64) throws {
        lock.lock()
        posted.append(Posted(event: event, tag: tag))
        if event.points { location = SmoothPointerPath.Point(x: event.x, y: event.y) }
        let hook = onPost
        lock.unlock()
        hook?(event, tag)
    }

    func pointerLocation() -> SmoothPointerPath.Point? {
        lock.lock()
        defer { lock.unlock() }
        return location
    }

    var events: [Posted] {
        lock.lock()
        defer { lock.unlock() }
        return posted
    }

    var presses: [HandEvent] { events.map(\.event).filter { $0.holds != nil } }
    var releases: [HandEvent] { events.map(\.event).filter { $0.letsGo != nil } }
    var moves: [HandEvent] { events.map(\.event).filter { $0.kind == .pointerMove } }
}

final class Box<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Value?

    var value: Value? {
        get {
            lock.lock()
            defer { lock.unlock() }
            return stored
        }
        set {
            lock.lock()
            stored = newValue
            lock.unlock()
        }
    }
}

/// Counts admissions the way `OperatorGuardHost.admission` answers them.
final class Admissions: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    func admit() -> GuardAdmission {
        lock.lock()
        count += 1
        lock.unlock()
        return .admitted
    }

    var admitted: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }
}

// MARK: - The shared fixtures

enum ReflexFixtures {
    static var root: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent() // test module
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // computer-use-macos
            .deletingLastPathComponent() // native
            .deletingLastPathComponent() // zerocode-shell
            .deletingLastPathComponent() // crates
            .appendingPathComponent("zerocode-core/fixtures")
    }

    static func limits() throws -> ReflexLimits {
        try ReflexContract.decodeLimits(Data(try Data(contentsOf: root.appendingPathComponent("reflex-contract/limits.json")).dropLast()))
    }

    static func perception() throws -> PerceptionLimits {
        try PerceptionSpecs.decodeLimits(Data(try Data(contentsOf: root.appendingPathComponent("game-state/limits.json")).dropLast()))
    }

    /// valid_basic's plan, changed by `change`, hashed again and validated
    /// under both tables.
    static func plan(_ change: (inout [String: Any]) -> Void = { _ in }) throws -> ValidatedReflexPlan {
        let raw = try Data(contentsOf: root.appendingPathComponent("reflex-contract/valid_basic.json"))
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: raw) as? [String: Any])
        var object = try XCTUnwrap(envelope["plan"] as? [String: Any])
        change(&object)
        object["plan_hash"] = ""
        let unhashed = try JSONDecoder().decode(ReflexPlan.self, from: JSONSerialization.data(withJSONObject: object))
        object["plan_hash"] = try ReflexContract.hash(unhashed)
        let wire = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])
        return try ReflexContract.decodeAndValidate(wire, limits: limits(), perception: perception())
    }

    /// A frame of run `run` on the 800 × 500 fixture extent, one point a pixel.
    static func frame(capture: UInt64, capturedNs: UInt64, owner: UInt64, stream: UInt64 = 1, run: String = "run") -> ReflexFrameFacts {
        ReflexCapture(
            displayId: "fixture",
            region: ReflexRoi(x: 0, y: 0, width: 800, height: 500, space: .pixel),
            pixelExtent: ReflexPixelExtent(width: 800, height: 500),
            pointTransform: ReflexPointTransform(origin_x: 0, origin_y: 0, points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
            orientation: .up,
            colorSpace: .srgb,
            status: .ready,
            dirty: true,
            captureGap: 0,
            deliveredHostNs: capturedNs,
            captureSeq: capture,
            repaintSeq: capture,
            streamEpoch: stream,
            geometryEpoch: 1,
            clockDomain: ReflexContract.hostUptimeClockDomain,
            capturedHostNs: capturedNs
        ).facts(runId: run, ownerEpoch: owner, planEpoch: 1)
    }

    /// The ball, seen on `frame` with `track` in `box` (inside valid_basic's
    /// 32 × 32 ROI), aimed at its centre.
    static func ball(on frame: ReflexFrameFacts, track: UInt64, box: ReflexRoi) -> ReflexObservation {
        ReflexObservation(
            detector_id: "ball",
            frame: ReflexFrameRef(frame)!,
            unknown: nil,
            value: 1,
            target: ReflexTarget(track_id: track, roi: box, point_x: box.x + box.width / 2, point_y: box.y + box.height / 2,
                                 velocity_x: 0, velocity_y: 0, uncertainty: 1),
            scale: ReflexScale(numerator: 1, denominator: 1),
            cells: nil,
            samples: 64
        )
    }

    static func seen(_ frame: ReflexFrameFacts, _ sightings: ReflexObservation...) -> ReflexSightings.Seen {
        ReflexSightings.Seen(frame: frame, byDetector: Dictionary(uniqueKeysWithValues: sightings.map { ($0.detector_id, $0) }))
    }
}

/// One click or move leaf on a hand in virtual time.
struct LeafRig {
    let clock = FakeClock()
    let poster = RecordingPoster()
    let sleeper: ScriptedSleeper
    let hand: OperatorHand
    let token: OperatorHand.Token
    let sightings = ReflexSightings()
    let admissions = Admissions()
    let limits: ReflexLimits
    let box = ReflexRoi(x: 8, y: 8, width: 16, height: 16, space: .pixel)

    init() throws {
        sleeper = ScriptedSleeper(clock: clock)
        hand = OperatorHand(poster: poster, clock: clock, sleeper: sleeper, tag: 0x7a6)
        token = try hand.acquire(.reflex("run"))
        limits = try ReflexFixtures.limits()
    }

    /// The frame the leaf is decided on: capture 10, a millisecond old.
    func decide() {
        let source = ReflexFixtures.frame(capture: 10, capturedNs: clock.nowNs() - 1_000_000, owner: token.id)
        sightings.publish(ReflexFixtures.seen(source, ReflexFixtures.ball(on: source, track: 5, box: box)))
    }

    /// A newer capture, just taken, showing `sighting` (or the ball where it was).
    func capture(_ seq: UInt64, track: UInt64 = 5, box moved: ReflexRoi? = nil) {
        let frame = ReflexFixtures.frame(capture: seq, capturedNs: clock.nowNs(), owner: token.id)
        sightings.publish(ReflexFixtures.seen(frame, ReflexFixtures.ball(on: frame, track: track, box: moved ?? box)))
    }

    func runner() throws -> ReflexLeafRunner {
        let plan = try ReflexFixtures.plan()
        let admissions = self.admissions
        return ReflexLeafRunner(
            runId: "run", hand: hand, token: token, limits: limits,
            style: try PointerStyle(plan.plan.pointer, limits: limits),
            sightings: sightings, admit: { admissions.admit() },
            fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000
        )
    }

    static let click = ReflexLeaf(ruleId: "follow", actionId: "click1", kind: .click, detector: "ball")
    static let move = ReflexLeaf(ruleId: "follow", actionId: "move1", kind: .move, detector: "ball")
}

final class ReflexRuntimeTests: XCTestCase {
    // MARK: red-first — the eight the brief names

    /// A hold with no frame coming and the request's road stalled behind the
    /// provider lock: the stop lets go of every key and button the hand
    /// pressed on the stopping thread itself, before it returns — no frame,
    /// socket or clock tick stands between them — and the hold ends refused.
    func test_stop_releases_held_input_when_capture_and_network_stall() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: GateSleeper(), tag: 0x5eed)
        let providerLock = NSLock()
        let holding = DispatchSemaphore(value: 0)
        let ended = DispatchSemaphore(value: 0)
        let refusal = Box<OperatorHand.Refusal>()
        let shift: UInt64 = 0x20000
        Thread.detachNewThread {
            providerLock.lock()
            defer { providerLock.unlock() }
            guard let token = try? hand.acquire(.request) else { return }
            try? hand.post(HandEvent(.key(code: 56, down: true, modifier: shift), flags: shift), by: token)
            try? hand.post(HandEvent(.key(code: 12, down: true, modifier: 0), flags: shift), by: token)
            holding.signal()
            do {
                try hand.sleep(untilNs: clock.nowNs() + 10_000_000_000, by: token)
            } catch let error as OperatorHand.Refusal {
                refusal.value = error
            } catch {}
            hand.relinquish(token)
            ended.signal()
        }
        XCTAssertEqual(holding.wait(timeout: .now() + 5), .success)
        XCTAssertFalse(providerLock.try(), "the request's road is stalled behind the hold")
        let before = clock.nowNs()
        let release = hand.stop(reason: StopReason.hotkey)
        XCTAssertEqual(release.released, [.key(12), .key(56)], "newest first: the key, then its modifier")
        XCTAssertEqual(poster.releases.map(\.kind), [
            .key(code: 12, down: false, modifier: 0),
            .key(code: 56, down: false, modifier: shift),
        ], "posted before stop returned")
        XCTAssertEqual(poster.releases.map(\.flags), [shift, 0], "the key lets go under the modifier still held")
        XCTAssertEqual(clock.nowNs(), before, "no time had to pass")
        XCTAssertEqual(ended.wait(timeout: .now() + 5), .success, "the hold ends at the stop")
        XCTAssertEqual(refusal.value, .stopped(StopReason.hotkey))
        XCTAssertThrowsError(try hand.acquire(.request), "nothing is pressed until a person resumes")
        hand.resume()

        // A reflex click in its fence, the frame source silent: the button is
        // let go of by the stop, and nothing more is pressed.
        let run = try hand.acquire(.reflex("run"))
        let fenced = DispatchSemaphore(value: 0)
        let clickEnded = DispatchSemaphore(value: 0)
        Thread.detachNewThread {
            try? hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 40, y: 50), by: run)
            fenced.signal()
            try? hand.sleep(untilNs: clock.nowNs() + 50_000_000, by: run)
            try? hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 40, y: 50), by: run)
            clickEnded.signal()
        }
        XCTAssertEqual(fenced.wait(timeout: .now() + 5), .success)
        XCTAssertEqual(hand.stop(reason: StopReason.signal).released, [.button(.left)])
        XCTAssertEqual(clickEnded.wait(timeout: .now() + 5), .success)
        XCTAssertEqual(poster.presses.filter { if case .buttonDown = $0.kind { return true } else { return false } }.count, 1, "no press after the stop")
        XCTAssertEqual(poster.releases.last?.kind, .buttonUp(.left, clickState: 1))
        XCTAssertEqual(hand.snapshot.held, [])
        hand.relinquish(run)
        hand.resume()
    }

    /// One writer: while a reflex run holds the hand no request takes it and
    /// no other token posts; every event carries the hand's one tag. The
    /// helper's own verbs hold the same hand, so a verb beside a run is
    /// refused before it posts anything.
    func test_legacy_actions_and_reflex_share_one_writer() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 41)
        let run = try hand.acquire(.reflex("r1"))
        XCTAssertThrowsError(try hand.acquire(.request)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .busy(.reflex("r1")))
        }
        let forged = OperatorHand.Token(id: run.id + 1, owner: .request)
        XCTAssertThrowsError(try hand.post(HandEvent(.pointerMove, x: 1, y: 1), by: forged)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .notHolder)
        }
        try hand.post(HandEvent(.pointerMove, x: 2, y: 2), by: run)
        hand.relinquish(run)
        let request = try hand.acquire(.request)
        XCTAssertThrowsError(try hand.acquire(.reflex("r2"))) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .busy(.request))
        }
        try hand.post(HandEvent(.key(code: 0, down: true, modifier: 0)), by: request)
        try hand.post(HandEvent(.key(code: 0, down: false, modifier: 0)), by: request)
        hand.relinquish(request)
        XCTAssertEqual(poster.events.map(\.tag), [41, 41, 41], "one writer, one stamp")
        XCTAssertThrowsError(try hand.post(HandEvent(.pointerMove, x: 3, y: 3), by: run), "a hold returned posts nothing")

        // The helper: its acting verbs take the one hand, and a run holds it.
        let helperRun = try OperatorHandHost.hand.acquire(.reflex("wiring"))
        defer { OperatorHandHost.hand.relinquish(helperRun) }
        let verbs: [(String, [String: JSONValue])] = [
            ("mouseMove", ["x": .number(1), "y": .number(1)]),
            ("mouseClick", ["x": .number(1), "y": .number(1)]),
            ("key", ["key": .string("a")]),
            ("holdKey", ["key": .string("a"), "ms": .number(10)]),
        ]
        for (method, params) in verbs {
            XCTAssertThrowsError(try Provider().handle(method: method, params: params), method) { error in
                XCTAssertEqual((error as? ProviderError)?.code, "hand_busy", method)
            }
        }
        XCTAssertEqual(OperatorHandHost.hand.snapshot.posted, 0, "refused before any event")
    }

    /// A leaf is admitted once however many events it posts, and every event
    /// reads the hold again: taken back mid-glide, no event follows; taken
    /// back in the press's fence, the button is let go of and nothing follows.
    func test_one_action_spends_once_and_every_micro_event_checks_revocation() throws {
        // A whole click: one admission for eleven events.
        let whole = try LeafRig()
        whole.decide()
        var seq: UInt64 = 10
        whole.sleeper.onSleep = { _, _ in
            seq += 1
            whole.capture(seq)
        }
        let done = try whole.runner().run(LeafRig.click, index: 0)
        XCTAssertEqual(done.outcome, .done)
        XCTAssertEqual(whole.admissions.admitted, 1)
        XCTAssertEqual(done.events, 12, "ten waypoints, a press and its release")
        XCTAssertEqual(whole.poster.moves.count, 10)

        // Taken back after four waypoints.
        let glide = try LeafRig()
        glide.decide()
        var glideSeq: UInt64 = 10
        glide.sleeper.onSleep = { index, _ in
            if index == 4 { glide.hand.revoke(glide.token, reason: "external_input") }
            glideSeq += 1
            glide.capture(glideSeq)
        }
        let cut = try glide.runner().run(LeafRig.click, index: 0)
        XCTAssertEqual(cut.outcome, .stopped)
        XCTAssertEqual(glide.admissions.admitted, 1)
        XCTAssertEqual(glide.poster.moves.count, 4, "no waypoint after the hold was taken back")
        XCTAssertEqual(glide.poster.presses.count, 0)

        // Taken back in the fence: the revoke lets go of the button.
        let fence = try LeafRig()
        fence.decide()
        var fenceSeq: UInt64 = 10
        fence.sleeper.onSleep = { index, _ in
            if index == 10 { fence.hand.revoke(fence.token, reason: "external_input") }
            fenceSeq += 1
            fence.capture(fenceSeq)
        }
        let fenced = try fence.runner().run(LeafRig.click, index: 0)
        XCTAssertEqual(fenced.outcome, .stopped)
        XCTAssertEqual(fence.poster.presses.map(\.kind), [.buttonDown(.left, clickState: 1)])
        XCTAssertEqual(fence.poster.releases.map(\.kind), [.buttonUp(.left, clickState: 1)], "let go by the revoke, once")
        XCTAssertEqual(fence.admissions.admitted, 1)

        // The writer itself reads the hold on every press; a release still goes.
        let writer = try LeafRig()
        try writer.hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 1, y: 1), by: writer.token)
        try writer.hand.post(HandEvent(.buttonUp(.left, clickState: 1), x: 1, y: 1), by: writer.token)
        try writer.hand.post(HandEvent(.buttonDown(.right, clickState: 1), x: 1, y: 1), by: writer.token)
        let taken = writer.hand.revoke(writer.token, reason: "external_input")
        XCTAssertEqual(taken.released, [.button(.right)])
        XCTAssertThrowsError(try writer.hand.post(HandEvent(.pointerMove, x: 2, y: 2), by: writer.token)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .revoked("external_input"))
        }
    }

    /// The press waits for a capture newer than the one the lease came from,
    /// still showing the same track with the pointer inside its hitbox: a
    /// target that moved off, another in its place, or no newer capture at
    /// all posts no button.
    func test_moving_target_is_revalidated_before_button_down() throws {
        func click(afterLastWaypoint change: @escaping (LeafRig) -> Void) throws -> (ReflexReceipt, LeafRig) {
            let rig = try LeafRig()
            rig.decide()
            var seq: UInt64 = 10
            rig.sleeper.onSleep = { _, _ in
                seq += 1
                rig.capture(seq)
            }
            rig.poster.onPost = { event, _ in
                if event.kind == .pointerMove, rig.poster.moves.count == 10 { change(rig) }
            }
            return (try rig.runner().run(LeafRig.click, index: 0), rig)
        }
        let far = ReflexRoi(x: 0, y: 0, width: 4, height: 4, space: .pixel)
        let (moved, movedRig) = try click { rig in rig.capture(90, box: far) }
        XCTAssertEqual(moved.outcome, .moved)
        XCTAssertEqual(movedRig.poster.presses.count, 0, "the target left the pointer: no press")

        let (replaced, replacedRig) = try click { rig in rig.capture(90, track: 6) }
        XCTAssertEqual(replaced.outcome, .moved)
        XCTAssertEqual(replacedRig.poster.presses.count, 0, "another target in its place: no press")

        let (held, heldRig) = try click { rig in rig.capture(90) }
        XCTAssertEqual(held.outcome, .done)
        XCTAssertEqual(heldRig.poster.presses.map(\.kind), [.buttonDown(.left, clickState: 1)])
        XCTAssertEqual(heldRig.poster.releases.map(\.kind), [.buttonUp(.left, clickState: 1)])

        // No capture after the one the lease came from: the proof runs out
        // before any event.
        let silent = try LeafRig()
        silent.decide()
        let starved = try silent.runner().run(LeafRig.click, index: 0)
        XCTAssertEqual(starved.outcome, .lease)
        XCTAssertEqual(silent.poster.events.count, 0)
    }

    /// Edges, not levels: a rule fires on true after it was armed by a false
    /// the run saw; the same capture read twice, a false on a frame the run
    /// never evaluated and an unknown arm nothing.
    func test_an_unseen_frame_does_not_retrigger_the_same_edge() throws {
        let plan = try ReflexFixtures.plan { object in
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["max_fires"] = 3
            rules[0]["cooldown_ms"] = 0
            rules[0]["predicate"] = ["op": "eq", "value": 1]
            object["rules"] = rules
        }
        var book = ReflexRuleBook(plan)
        func read(_ capture: UInt64, _ value: Int64?, at now: UInt64 = 0, free: Bool = true) -> String? {
            book.read(streamEpoch: 1, captureSeq: capture, values: value.map { ["ball": $0] } ?? [:], nowNs: now, handFree: free)?.id
        }
        XCTAssertEqual(read(1, 1), "follow", "armed at the start, true fires")
        XCTAssertNil(read(1, 1), "the same capture read twice")
        XCTAssertNil(read(3, 1), "capture 2 was never evaluated: its false, if any, arms nothing")
        XCTAssertNil(read(4, nil), "unknown does not re-arm")
        XCTAssertNil(read(5, 1))
        XCTAssertNil(read(6, 0), "a false the run saw re-arms")
        XCTAssertNil(read(7, 1, free: false), "the hand is busy: nothing fires, nothing is used up")
        XCTAssertEqual(read(8, 1), "follow", "still armed when the hand is free")
        XCTAssertNil(read(9, 0))
        XCTAssertEqual(read(10, 1), "follow", "the third fire")
        XCTAssertNil(read(11, 0))
        XCTAssertNil(read(12, 1), "max_fires spent")
        XCTAssertNil(book.read(streamEpoch: 1, captureSeq: 2, values: ["ball": 1], nowNs: 0, handFree: true), "an older capture is not new")
    }

    /// A run asking for more frames changes the stream's rate in place: the
    /// window's next eye start with its own table keeps the eye (and every
    /// reader's cursor), and the run giving it back lowers it in place again.
    func test_screen_fps_upgrade_does_not_reset_other_eye_consumers() throws {
        let table = try XCTUnwrap(EyeConfig(framesPerSecond: 30, changesKept: 512, idleStopMs: 30_000, firstFrameMs: 2_000,
                                            ocrCellPoints: 24, ocrMarginPoints: 4, ocrMaxShare: 0.5, ocrMaxPieces: 4))
        let limits = try ReflexFixtures.limits()
        var readers = EyeReaders()
        XCTAssertEqual(readers.start(table), .restart, "the first start opens the eye")
        var ring = EyeRing(config: table)
        ring.note(rects: [CGRect(x: 0, y: 0, width: 1, height: 1)], atMs: 1)
        let lookCursor = ring.latest
        XCTAssertEqual(readers.add("run", framesPerSecond: Int(limits.frames_per_second)), .rate(60), "in place, not a restart")
        XCTAssertTrue(readers.held, "a run keeps the eye open past the idle time")
        XCTAssertEqual(readers.start(table), .keep, "the window's own eye start keeps the eye and its rate")
        XCTAssertEqual(readers.framesPerSecond, 60)
        XCTAssertEqual(readers.add("another", framesPerSecond: 30), .keep)
        ring.note(rects: [CGRect(x: 1, y: 1, width: 1, height: 1)], atMs: 2)
        XCTAssertEqual(ring.changes(after: lookCursor, fromMs: 0).changes.map(\.seq), [2], "the look's cursor reads on")
        XCTAssertEqual(readers.remove("run"), .rate(30), "given back in place")
        XCTAssertEqual(readers.remove("another"), .keep)
        XCTAssertFalse(readers.held)
        let other = try XCTUnwrap(EyeConfig(framesPerSecond: 20, changesKept: 512, idleStopMs: 30_000, firstFrameMs: 2_000,
                                            ocrCellPoints: 24, ocrMarginPoints: 4, ocrMaxShare: 0.5, ocrMaxPieces: 4))
        XCTAssertEqual(readers.start(other), .restart, "only another table from the window reopens the eye")
    }

    /// The hand's own events carry its tag and never pause a run; anyone
    /// else's — untagged, or tagged from another process — pauses it and lets
    /// go of what it held. A monitor that never hears the hand's echo cannot
    /// start a run.
    func test_tagged_self_events_do_not_pause_and_unknown_events_do() throws {
        let me = Int64(getpid())
        XCTAssertEqual(InputOrigin.of(userData: 99, sourcePid: me, handTag: 99, handPid: me), .hand)
        XCTAssertEqual(InputOrigin.of(userData: 0, sourcePid: me, handTag: 99, handPid: me), .other, "untagged")
        XCTAssertEqual(InputOrigin.of(userData: 99, sourcePid: me + 1, handTag: 99, handPid: me), .other, "the tag from another process")
        XCTAssertEqual(InputOrigin.of(userData: 98, sourcePid: me, handTag: 99, handPid: me), .other)
        XCTAssertEqual(InputOrigin.of(userData: 0, sourcePid: me, handTag: 0, handPid: me), .other, "a zero tag is everyone's")

        let rig = try SessionRig()
        try rig.session.start()
        XCTAssertEqual(rig.session.status.state, .running)
        XCTAssertEqual(rig.session.status.monitor, .hearing, "the hand's own echo proved the monitor hears")
        XCTAssertTrue(rig.poster.events.allSatisfy { $0.tag == rig.hand.tag }, "every event stamped")
        rig.monitor.hearHand()
        rig.monitor.settle()
        XCTAssertEqual(rig.session.status.state, .running, "the hand's own event pauses nothing")
        rig.monitor.hearOther()
        rig.monitor.settle()
        XCTAssertEqual(rig.session.status.state, .paused("external_input"))
        XCTAssertEqual(rig.hand.snapshot.revoked, "external_input", "the run's hold is taken back")
        XCTAssertThrowsError(try rig.hand.acquire(.request), "a paused run still holds the hand")
        rig.session.stop(reason: StopReason.request)
        XCTAssertEqual(rig.hand.snapshot.holder, nil)

        let deaf = try SessionRig(echo: false)
        XCTAssertThrowsError(try deaf.session.start()) { error in
            XCTAssertEqual(error as? ReflexRunError, .monitorDeaf("the monitor did not hear the hand's own echo"))
        }
        XCTAssertEqual(deaf.hand.snapshot.holder, nil, "a run that cannot hear gives the hand back")
    }

    /// Receipts are offered, never waited on: a reader that stopped reading
    /// fills the queue, the next leaf is refused instead of blocked, and the
    /// hand still lets go of what it holds.
    func test_slow_evidence_consumer_cannot_block_the_hand() throws {
        let receipts = ReflexReceipts(capacity: 2)
        let consumerBusy = DispatchSemaphore(value: 0)
        let consumerRelease = DispatchSemaphore(value: 0)
        Thread.detachNewThread {
            _ = receipts.drain(limit: 0)
            consumerBusy.signal()
            _ = consumerRelease.wait(timeout: .now() + 10) // the reader is stuck elsewhere
        }
        XCTAssertEqual(consumerBusy.wait(timeout: .now() + 5), .success)
        let leaves = (0..<5).map { ReflexLeaf(ruleId: "follow", actionId: "a\($0)", kind: .move, detector: "ball") }
        var next: UInt64 = 0
        var ran: [String] = []
        let started = Date()
        let count = ReflexSession.runFire(leaves, receipts: receipts, acting: { true }, next: &next) { leaf, index in
            ran.append(leaf.actionId)
            return ReflexReceipt(ruleId: leaf.ruleId, actionId: leaf.actionId, leafIndex: index, outcome: .done, targetId: nil,
                                 sourceCapture: nil, decidedHostNs: nil, admittedHostNs: nil, captureWaitNs: 0,
                                 firstEventHostNs: nil, downHostNs: nil, upHostNs: nil, endedHostNs: 0, events: 1)
        }
        XCTAssertLessThan(Date().timeIntervalSince(started), 1, "never waited on the reader")
        XCTAssertEqual(count, 2)
        XCTAssertEqual(ran, ["a0", "a1"], "the third leaf is refused, not blocked")
        XCTAssertEqual(receipts.refused, 1)
        XCTAssertEqual(receipts.pending, 2)
        // The hand is the hand's: a full queue does not stand between a stop and the release.
        let rig = try LeafRig()
        try rig.hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 3, y: 3), by: rig.token)
        XCTAssertEqual(rig.hand.stop(reason: StopReason.hotkey).released, [.button(.left)])
        consumerRelease.signal()
        XCTAssertEqual(receipts.drain(limit: 10).map(\.actionId), ["a0", "a1"])
    }

    // MARK: the helper's roads

    /// The stop answers while an action in flight holds the provider lock.
    func test_the_stop_road_does_not_wait_for_the_provider_lock() throws {
        let providerLock = NSLock()
        providerLock.lock()
        let answered = DispatchSemaphore(value: 0)
        let answer = Box<[String: Any]>()
        Thread.detachNewThread {
            guard let request = try? JSONDecoder().decode(Request.self, from: Data(#"{"id":7,"method":"stop"}"#.utf8)) else { return }
            answer.value = handleRequest(provider: Provider(), lock: providerLock, request: request, expectedToken: nil, authorizedPeer: true) as? [String: Any]
            answered.signal()
        }
        let result = answered.wait(timeout: .now() + 5)
        providerLock.unlock()
        XCTAssertEqual(result, .success, "the stop did not wait behind the action")
        let body = try XCTUnwrap(answer.value?["result"] as? [String: Any])
        XCTAssertEqual(body["stopped"] as? Bool, true)
        OperatorGuardHost.resume(resetBudget: false)
        XCTAssertNil(OperatorHandHost.hand.snapshot.stopped)
    }

    /// A reflex start is validated by the helper under the tables the window
    /// sent — never numbers of its own — before it reaches the eye or the hand.
    func test_a_reflex_start_reads_the_windows_tables_and_refuses_what_it_cannot_run() throws {
        let fixtures = ReflexFixtures.root
        let limits = String(decoding: try Data(contentsOf: fixtures.appendingPathComponent("reflex-contract/limits.json")).dropLast(), as: UTF8.self)
        let perception = String(decoding: try Data(contentsOf: fixtures.appendingPathComponent("game-state/limits.json")).dropLast(), as: UTF8.self)
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: fixtures.appendingPathComponent("reflex-contract/valid_basic.json"))) as? [String: Any])
        let wire = String(decoding: try JSONSerialization.data(withJSONObject: try XCTUnwrap(envelope["plan"]), options: [.sortedKeys, .withoutEscapingSlashes]), as: UTF8.self)
        let eye: JSONValue = .object([
            "framesPerSecond": .number(30), "changesKept": .number(512), "idleStopMs": .number(30_000), "firstFrameMs": .number(2_000),
            "ocrCellPoints": .number(24), "ocrMarginPoints": .number(4), "ocrMaxShare": .number(0.5), "ocrMaxPieces": .number(4),
        ])
        func start(_ change: (inout [String: JSONValue]) -> Void) -> String? {
            var params: [String: JSONValue] = [
                "runId": .string("wiring"), "plan": .string(wire), "limits": .string(limits),
                "perception": .string(perception), "eye": eye, "display": .number(0),
            ]
            change(&params)
            do {
                _ = try Provider().handle(method: "reflexStart", params: params)
                return nil
            } catch let error as ProviderError {
                return error.code
            } catch {
                return "\(error)"
            }
        }
        XCTAssertEqual(start { _ in }, "unsupported_capability", "no kernel in this build: refused before the eye or the hand")
        XCTAssertEqual(start { $0["plan"] = .string(wire.replacingOccurrences(of: "\"version\":2", with: "\"version\":1")) }, "invalid_argument")
        XCTAssertEqual(start { $0["limits"] = .string(limits.replacingOccurrences(of: "\"pointer_tick_ns\":8000000", with: "\"pointer_tick_ns\":7")) }, "invalid_argument",
                       "a table whose longest glide overflows one lease is refused, not trimmed")
        XCTAssertEqual(start { $0["limits"] = nil }, "invalid_argument", "no table, no run")
        XCTAssertEqual(start { $0["runId"] = .string("") }, "invalid_argument")
        XCTAssertEqual(OperatorHandHost.hand.snapshot.holder, nil)
    }
}

// MARK: - A run with fakes

/// A frame source the test fills; its pixels are never read by the fake kernel.
final class ScriptedFrames: ReflexFrameSource, @unchecked Sendable {
    private let lock = NSLock()
    private var capture: ReflexCapture?
    private var wake: (@Sendable () -> Void)?

    func newest() -> ReflexFrameLoan? {
        lock.lock()
        defer { lock.unlock() }
        return capture.map { ReflexFrameLoan(capture: $0) { _ in false } }
    }

    func onCapture(_ wake: (@Sendable () -> Void)?) {
        lock.lock()
        self.wake = wake
        lock.unlock()
    }
}

/// A kernel that answers nothing: these runs are about the hand and the monitor.
struct SilentKernel: ReflexPerceptionKernel {
    final class Session: ReflexPerceptionSession {
        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, budget: inout ReflexPerceptionBudget) -> [ReflexObservation] { [] }
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession { Session() }
}

/// A monitor that hears what the hand posts (its tag and this process) and
/// whatever the test says someone else did, on its own queue as a tap does.
final class EchoMonitor: ReflexInputMonitor, @unchecked Sendable {
    private let lock = NSLock()
    private let queue = DispatchQueue(label: "test.monitor")
    private let echo: Bool
    private var heard: (@Sendable (InputOrigin) -> Void)?

    init(echo: Bool) { self.echo = echo }

    func start(heard: @escaping @Sendable (InputOrigin) -> Void, interrupted: @escaping @Sendable (String) -> Void) throws {
        lock.lock()
        self.heard = echo ? heard : nil
        lock.unlock()
    }

    func stop() {
        lock.lock()
        heard = nil
        lock.unlock()
    }

    func deliver(_ origin: InputOrigin) {
        lock.lock()
        let heard = self.heard
        lock.unlock()
        queue.async { heard?(origin) }
    }

    func hearHand() { deliver(.hand) }
    func hearOther() { deliver(.other) }
    func settle() { queue.sync {} }
}

struct SessionRig {
    let poster = RecordingPoster()
    let hand: OperatorHand
    let monitor: EchoMonitor
    let session: ReflexSession

    init(echo: Bool = true) throws {
        hand = OperatorHand(poster: poster, clock: HostUptimeClock(), sleeper: SemaphoreSleeper(), tag: 0x7e57)
        let monitor = EchoMonitor(echo: echo)
        self.monitor = monitor
        let hand = self.hand
        poster.onPost = { _, tag in
            // The tap hears the event with the stamp it was posted with, from this process.
            monitor.deliver(InputOrigin.of(userData: tag, sourcePid: Int64(getpid()), handTag: hand.tag, handPid: Int64(getpid())))
        }
        session = ReflexSession(
            settings: ReflexSession.Settings(runId: "run", plan: try ReflexFixtures.plan(), limits: try ReflexFixtures.limits(),
                                             perception: try ReflexFixtures.perception(), planEpoch: 1),
            hand: hand, source: ScriptedFrames(), kernel: SilentKernel(), monitor: monitor,
            admit: { .admitted }, fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000
        )
    }
}
