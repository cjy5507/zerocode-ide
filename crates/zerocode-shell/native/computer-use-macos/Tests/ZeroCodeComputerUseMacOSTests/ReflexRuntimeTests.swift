import AppKit
import CoreGraphics
import CoreVideo
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

    struct Refused: Error {}

    private let lock = NSLock()
    private var posted: [Posted] = []
    private var location = SmoothPointerPath.Point(x: 0, y: 0)
    /// Runs after each post, outside the poster's lock (but inside the hand's:
    /// it must not call the hand).
    var onPost: ((HandEvent, Int64) -> Void)?
    /// An event the platform could not make or post: it is not recorded and
    /// the hand hears the error.
    var refuse: ((HandEvent) -> Bool)?

    func post(_ event: HandEvent, tag: Int64) throws {
        lock.lock()
        if refuse?(event) == true {
            lock.unlock()
            throw Refused()
        }
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

    /// Someone else moved the pointer, and no monitor has heard it yet (a
    /// tap hears it a moment later): nothing is recorded as posted.
    func movePointer(to point: SmoothPointerPath.Point) {
        lock.lock()
        location = point
        lock.unlock()
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
        try ReflexContract.decodeAndValidate(wire(change), limits: limits(), perception: perception())
    }

    /// valid_basic's plan, changed by `change` and hashed again, as the
    /// window sends it: canonical bytes.
    static func wire(_ change: (inout [String: Any]) -> Void = { _ in }) throws -> Data {
        let raw = try Data(contentsOf: root.appendingPathComponent("reflex-contract/valid_basic.json"))
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: raw) as? [String: Any])
        var object = try XCTUnwrap(envelope["plan"] as? [String: Any])
        change(&object)
        object["plan_hash"] = ""
        let unhashed = try JSONDecoder().decode(ReflexPlan.self, from: JSONSerialization.data(withJSONObject: object))
        object["plan_hash"] = try ReflexContract.hash(unhashed)
        return try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])
    }

    /// A frame of run `run` on the 800 × 500 fixture extent, one point a pixel.
    static func frame(capture: UInt64, capturedNs: UInt64, owner: UInt64, stream: UInt64 = 1, run: String = "run") -> ReflexFrameFacts {
        self.capture(capture, capturedNs: capturedNs, stream: stream).facts(runId: run, ownerEpoch: owner, planEpoch: 1)
    }

    /// Capture `seq` of the 800 × 500 fixture extent, one point a pixel, as a
    /// platform adapter writes it: ready unless `status` says otherwise, its
    /// capture time unknown when `capturedNs` is nil.
    static func capture(_ seq: UInt64, capturedNs: UInt64?, status: ReflexFrameStatus = .ready, stream: UInt64 = 1) -> ReflexCapture {
        ReflexCapture(
            displayId: "fixture",
            region: ReflexRoi(x: 0, y: 0, width: 800, height: 500, space: .pixel),
            pixelExtent: ReflexPixelExtent(width: 800, height: 500),
            pointTransform: ReflexPointTransform(origin_x: 0, origin_y: 0, points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
            orientation: .up,
            colorSpace: .srgb,
            status: status,
            dirty: true,
            captureGap: 0,
            deliveredHostNs: capturedNs,
            captureSeq: seq,
            repaintSeq: seq,
            streamEpoch: stream,
            geometryEpoch: 1,
            clockDomain: ReflexContract.hostUptimeClockDomain,
            capturedHostNs: capturedNs
        )
    }

    /// valid_basic with its macro one click on the ball, its rule firing on
    /// the ball being there (an edge each time it comes back) at most
    /// `maxFires` times.
    static func clickPlan(maxFires: Int = 1) throws -> ValidatedReflexPlan {
        try plan { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [["id": "click1", "kind": "click", "target": "ball"]]]]
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["max_fires"] = maxFires
            rules[0]["cooldown_ms"] = 0
            rules[0]["predicate"] = ["op": "eq", "value": 1]
            object["rules"] = rules
        }
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
            fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
            boundary: PermitEverywhere()
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
        XCTAssertNotNil(done.firstEventFrameHostNs, "the newer capture that let the first event go is on the receipt")
        XCTAssertGreaterThan(done.firstEventFrameHostNs ?? 0, done.decidedHostNs ?? .max)
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
                                 firstEventHostNs: nil, firstEventFrameHostNs: nil, downHostNs: nil, upHostNs: nil, endedHostNs: 0, events: 1)
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
            admit: { .admitted }, fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
            boundary: PermitEverywhere()
        )
    }
}

/// A boundary that asks nothing: for runs whose test is about the hand, the
/// frames or the stop, never about where a press may land. Tests only — the
/// helper's runs always carry `DesktopRunBoundary`.
struct PermitEverywhere: ReflexInputBoundary {
    func refusal(_ input: ReflexLeaseInput, at point: SmoothPointerPath.Point) -> String? { nil }
}

/// A boundary that allows everything and keeps where it was asked; the
/// test's step runs inside an ask, before its answer — as the window server's
/// answer takes its time.
final class AskedBoundary: ReflexInputBoundary, @unchecked Sendable {
    private let lock = NSLock()
    private var asked: [SmoothPointerPath.Point] = []
    /// Runs inside the `n`th ask, counted from 1.
    var during: ((Int) -> Void)?

    func refusal(_ input: ReflexLeaseInput, at point: SmoothPointerPath.Point) -> String? {
        lock.lock()
        asked.append(point)
        let ask = asked.count
        let step = during
        lock.unlock()
        step?(ask)
        return nil
    }

    var points: [SmoothPointerPath.Point] {
        lock.lock()
        defer { lock.unlock() }
        return asked
    }
}

// MARK: - A run the test steps (the edges astra's review found)

/// Waits in real time until `condition` holds; false when it never did.
func eventually(within seconds: Double = 5, _ condition: () -> Bool) -> Bool {
    let end = Date().addingTimeInterval(seconds)
    while !condition() {
        if Date() > end { return false }
        usleep(200)
    }
    return true
}

/// A door one thread waits at while it is shut; the test hears who arrived
/// and opens it. Real time bounds every wait, so a failing test ends.
final class Gate: @unchecked Sendable {
    private let lock = NSLock()
    private var shut = false
    private let arrived = DispatchSemaphore(value: 0)
    private let opened = DispatchSemaphore(value: 0)

    func close() {
        lock.lock()
        shut = true
        lock.unlock()
    }

    /// Wait here while the door is shut.
    func pass() {
        lock.lock()
        let wait = shut
        lock.unlock()
        guard wait else { return }
        arrived.signal()
        _ = opened.wait(timeout: .now() + 10)
    }

    func awaitArrival(seconds: Double = 5) -> Bool {
        arrived.wait(timeout: .now() + seconds) == .success
    }

    func open() {
        lock.lock()
        let was = shut
        shut = false
        lock.unlock()
        if was { opened.signal() }
    }
}

/// A frame source the test publishes into, one capture at a time. It keeps
/// which capture each of the run's reads took, so a test knows when one was
/// read and done with: taken, then read again — itself or a newer one, which
/// another thread may have published meanwhile. Every capture lends one blank
/// BGRA buffer of the fixture's extent. The run's teardown (`onCapture(nil)`)
/// waits at `teardown` while the test holds it shut.
final class SteppedFrames: ReflexFrameSource, @unchecked Sendable {
    static let width = 800
    static let height = 500
    nonisolated(unsafe) static let blank: UnsafeMutableRawPointer = {
        let bytes = width * height * 4
        let pointer = UnsafeMutableRawPointer.allocate(byteCount: bytes, alignment: 16)
        pointer.initializeMemory(as: UInt8.self, repeating: 0, count: bytes)
        return pointer
    }()

    let teardown = Gate()
    private let lock = NSLock()
    private var capture: ReflexCapture?
    private var wake: (@Sendable () -> Void)?
    private var reads: [UInt64] = []

    func publish(_ next: ReflexCapture) {
        lock.lock()
        capture = next
        let wake = self.wake
        lock.unlock()
        wake?()
    }

    /// Wake the run as a capture would, with nothing new.
    func poke() {
        lock.lock()
        let wake = self.wake
        lock.unlock()
        wake?()
    }

    /// How many of the run's reads took capture `seq` or a newer one.
    func taken(_ seq: UInt64) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return reads.filter { $0 >= seq }.count
    }

    func newest() -> ReflexFrameLoan? {
        lock.lock()
        let capture = self.capture
        if let capture { reads.append(capture.captureSeq) }
        lock.unlock()
        let pixels = ReflexPixels(base: UnsafeRawPointer(Self.blank), width: Self.width, height: Self.height, bytesPerRow: Self.width * 4)
        return capture.map { ReflexFrameLoan(capture: $0) { body in
            body(pixels)
            return true
        } }
    }

    func onCapture(_ wake: (@Sendable () -> Void)?) {
        if wake == nil { teardown.pass() }
        lock.lock()
        self.wake = wake
        lock.unlock()
    }
}

/// A kernel that answers what the test's scene says about each capture: the
/// ball by track and hitbox, nothing there, or nothing known.
final class SceneKernel: ReflexPerceptionKernel, @unchecked Sendable {
    enum Sight {
        case ball(track: UInt64, box: ReflexRoi)
        case absent
        case unknown
    }

    private let lock = NSLock()
    private var sights: [UInt64: Sight] = [:]

    func show(_ sight: Sight, on capture: UInt64) {
        lock.lock()
        sights[capture] = sight
        lock.unlock()
    }

    fileprivate func sight(_ capture: UInt64) -> Sight {
        lock.lock()
        defer { lock.unlock() }
        return sights[capture] ?? .unknown
    }

    final class Session: ReflexPerceptionSession {
        let kernel: SceneKernel
        init(kernel: SceneKernel) { self.kernel = kernel }

        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
            guard let ref = ReflexFrameRef(frame) else { return [] }
            let unity = ReflexScale(numerator: 1, denominator: 1)
            switch kernel.sight(frame.capture_seq) {
            case let .ball(track, box):
                return [ReflexFixtures.ball(on: frame, track: track, box: box)]
            case .absent:
                return [ReflexObservation(detector_id: "ball", frame: ref, unknown: nil, value: 0, target: nil, scale: unity, cells: nil, samples: 64)]
            case .unknown:
                return [ReflexObservation(detector_id: "ball", frame: ref, unknown: .occluded, value: nil, target: nil, scale: unity, cells: nil, samples: 64)]
            }
        }
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        Session(kernel: self)
    }
}

/// Hears the hand's own events on the posting thread — as the tap would a
/// moment later — so a run on a test clock proves its monitor at once. Its
/// stop waits at `teardown` while the test holds it shut.
final class SyncEchoMonitor: ReflexInputMonitor, @unchecked Sendable {
    /// A start waits here while the test holds it shut.
    let starting = Gate()
    let teardown = Gate()
    private let lock = NSLock()
    private var heard: (@Sendable (InputOrigin) -> Void)?

    func start(heard: @escaping @Sendable (InputOrigin) -> Void, interrupted: @escaping @Sendable (String) -> Void) throws {
        starting.pass()
        lock.lock()
        self.heard = heard
        lock.unlock()
    }

    func stop() {
        teardown.pass()
        lock.lock()
        heard = nil
        lock.unlock()
    }

    func hear(_ origin: InputOrigin) {
        lock.lock()
        let heard = self.heard
        lock.unlock()
        heard?(origin)
    }
}

/// A reflex run on fakes that the test steps: the leaf's every sleep runs the
/// test's step and then moves the clock to its deadline; a capture is
/// published and read by the run's evaluator before the step goes on; the
/// monitor hears the hand's echo on the posting thread. Nothing waits on real
/// time but the rendezvous themselves.
final class LiveRig: @unchecked Sendable {
    /// The ball where the leaf aims, inside valid_basic's 32 × 32 ROI.
    static let box = ReflexRoi(x: 8, y: 8, width: 16, height: 16, space: .pixel)

    let clock = FakeClock(DispatchTime.now().uptimeNanoseconds)
    let poster = RecordingPoster()
    let sleeper: ScriptedSleeper
    let hand: OperatorHand
    let frames = SteppedFrames()
    let kernel = SceneKernel()
    let monitor = SyncEchoMonitor()
    private(set) var session: ReflexSession!
    /// Runs after each event the poster took, on the posting thread (inside
    /// the hand's lock: it must not call the hand).
    var afterPost: ((HandEvent) -> Void)?
    private let lock = NSLock()
    private var seq: UInt64 = 0

    enum Captured {
        case now
        case unknown
        /// This long before the newest capture published before it.
        case beforeLast(UInt64)
    }

    private var lastCapturedNs: UInt64?

    /// A run of `plan` reading `frames`, or `source` when one is given; its
    /// presses held to `boundary`.
    init(plan: ValidatedReflexPlan, boundary: any ReflexInputBoundary = PermitEverywhere(), source: (any ReflexFrameSource)? = nil) throws {
        sleeper = ScriptedSleeper(clock: clock)
        hand = OperatorHand(poster: poster, clock: clock, sleeper: sleeper, tag: 0x5e55)
        let hand = self.hand
        let monitor = self.monitor
        let me = Int64(getpid())
        poster.onPost = { [weak self] event, tag in
            monitor.hear(InputOrigin.of(userData: tag, sourcePid: me, handTag: hand.tag, handPid: me))
            self?.afterPost?(event)
        }
        session = try makeSession(plan, boundary: boundary, source: source ?? frames)
    }

    /// The run itself: the one place the test spells `ReflexSession`'s initializer.
    private func makeSession(_ plan: ValidatedReflexPlan, boundary: any ReflexInputBoundary, source: any ReflexFrameSource) throws -> ReflexSession {
        ReflexSession(
            settings: ReflexSession.Settings(runId: "run", plan: plan, limits: try ReflexFixtures.limits(),
                                             perception: try ReflexFixtures.perception(), planEpoch: 1),
            hand: hand, source: source, kernel: kernel, monitor: monitor,
            admit: { .admitted }, fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
            boundary: boundary
        )
    }

    /// Wait until the run's evaluator took capture `seq` (or a newer one) and
    /// came back for another: it is done with it.
    func settle(_ seq: UInt64, taken: (UInt64) -> Int, poke: () -> Void) {
        guard eventually({ taken(seq) >= 1 }) else { return }
        poke()
        _ = eventually { taken(seq) >= 2 }
    }

    /// Publish the next capture showing `sight` — ready and captured now
    /// unless said otherwise — and return once the run's evaluator took it and
    /// came back for another: it is done with it.
    @discardableResult
    func frame(_ sight: SceneKernel.Sight, status: ReflexFrameStatus = .ready, captured: Captured = .now) -> UInt64 {
        lock.lock()
        seq += 1
        let seq = self.seq
        let capturedNs: UInt64?
        switch captured {
        case .now: capturedNs = clock.nowNs()
        case .unknown: capturedNs = nil
        case let .beforeLast(ns): capturedNs = (lastCapturedNs ?? clock.nowNs()) - ns
        }
        if let capturedNs, status == .ready, captured == .now { lastCapturedNs = capturedNs }
        lock.unlock()
        kernel.show(sight, on: seq)
        frames.publish(ReflexFixtures.capture(seq, capturedNs: capturedNs, status: status))
        settle(seq, taken: frames.taken, poke: frames.poke)
        return seq
    }

    /// The leaves the run finished, their receipts drained in order.
    func receipts(_ count: Int, seconds: Double = 5) -> [ReflexReceipt] {
        var drained: [ReflexReceipt] = []
        _ = eventually(within: seconds) {
            drained += session.receipts.drain(limit: count - drained.count)
            return drained.count >= count
        }
        return drained
    }

    var downs: Int { poster.presses.filter { if case .buttonDown = $0.kind { return true } else { return false } }.count }
    var ups: Int { poster.releases.filter { if case .buttonUp = $0.kind { return true } else { return false } }.count }
}

extension LiveRig.Captured: Equatable {}

final class ReflexRunBoundaryTests: XCTestCase {
    // MARK: R1b — a refused release is settled, and the run presses nothing more

    /// The platform refuses the release after a reflex click's fence: the hold
    /// is taken back and the run pauses saying why, so the ball coming back
    /// fires nothing and presses nothing; the hand counts the release it could
    /// not confirm and a request cannot take the hand until a person resumes.
    func test_a_run_whose_release_was_refused_presses_nothing_more() throws {
        let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(maxFires: 2))
        var refusedOnce = false
        rig.poster.refuse = { event in
            guard !refusedOnce, case .buttonUp = event.kind else { return false }
            refusedOnce = true
            return true
        }
        var track: UInt64 = 5
        rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: track, box: LiveRig.box)) }
        try rig.session.start()
        rig.frame(.ball(track: track, box: LiveRig.box))
        let first = rig.receipts(1)
        XCTAssertEqual(first.count, 1)
        XCTAssertEqual(rig.downs, 1)
        XCTAssertEqual(rig.hand.snapshot.unconfirmedReleases, 1, "the release the platform refused is counted")
        XCTAssertEqual(rig.hand.snapshot.held, [], "settled: nothing is called held that the hand could not let go of")
        XCTAssertEqual(rig.hand.snapshot.revoked, "release_unconfirmed", "the hold is taken back")
        XCTAssertTrue(eventually { rig.session.status.state == .paused("release_unconfirmed") }, "the run says why it stopped acting: \(rig.session.status.state)")

        // The ball goes and comes back: an edge the rule would fire on.
        track = 7
        rig.frame(.absent)
        rig.frame(.ball(track: track, box: LiveRig.box))
        XCTAssertEqual(rig.session.status.fires, 1, "a run that could not let go fires no more")
        XCTAssertFalse(eventually(within: 0.3) { rig.downs > 1 }, "no new press")
        rig.session.stop(reason: StopReason.request)
        XCTAssertThrowsError(try rig.hand.acquire(.request)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .releaseUnconfirmed, "until a person resumes")
        }
        rig.hand.resume()
        XCTAssertNoThrow(try rig.hand.acquire(.request))
    }

    // MARK: R2 — a refused newer frame takes back the ready frame's permission

    enum Refusal: CaseIterable {
        case interrupted
        case timeUnknown
        case timeBackwards

        func publish(on rig: LiveRig) {
            switch self {
            case .interrupted: rig.frame(.ball(track: 5, box: LiveRig.box), status: .interrupted)
            case .timeUnknown: rig.frame(.ball(track: 5, box: LiveRig.box), captured: .unknown)
            case .timeBackwards: rig.frame(.ball(track: 5, box: LiveRig.box), captured: .beforeLast(1_000_000))
            }
        }
    }

    /// Capture 10 decides a click, capture 11 lets its first waypoint go, and
    /// within the frame age a newer capture comes that the run refuses — the
    /// stream interrupted, its time unknown, its time before capture 11's. No
    /// waypoint and no press follows on capture 11: the refusal took it back.
    /// The same refusal before the press stops the press; inside the press's
    /// fence the button is still let go of.
    func test_a_refused_newer_frame_takes_back_the_ready_frames_permission() throws {
        for refusal in Refusal.allCases {
            // Between two waypoints.
            let glide = try LiveRig(plan: try ReflexFixtures.clickPlan())
            var eventsBefore = 0
            glide.sleeper.onSleep = { index, _ in
                switch index {
                case 0: glide.frame(.ball(track: 5, box: LiveRig.box))
                case 1:
                    eventsBefore = glide.poster.events.count
                    refusal.publish(on: glide)
                default: break
                }
            }
            try glide.session.start()
            glide.frame(.ball(track: 5, box: LiveRig.box))
            let cut = glide.receipts(1)
            XCTAssertEqual(glide.poster.events.count, eventsBefore, "\(refusal): nothing goes on the frame the refusal took back")
            XCTAssertEqual(glide.downs, 0, "\(refusal)")
            XCTAssertEqual(cut.first?.outcome.rawValue, "evidence", "\(refusal)")
            glide.session.stop(reason: StopReason.request)

            // After the last waypoint, before the press.
            let press = try LiveRig(plan: try ReflexFixtures.clickPlan())
            press.sleeper.onSleep = { _, _ in press.frame(.ball(track: 5, box: LiveRig.box)) }
            var waypoints = 0
            press.afterPost = { event in
                guard event.kind == .pointerMove else { return }
                waypoints += 1
                // The monitor's echo was the first move; the glide's tenth is the last.
                if waypoints == 11 { refusal.publish(on: press) }
            }
            try press.session.start()
            press.frame(.ball(track: 5, box: LiveRig.box))
            let stopped = press.receipts(1)
            XCTAssertEqual(press.downs, 0, "\(refusal): no press on the frame the refusal took back")
            XCTAssertEqual(stopped.first?.outcome.rawValue, "evidence", "\(refusal)")
            press.session.stop(reason: StopReason.request)

            // Inside the fence: the refusal stops nothing that is the hand's to let go of.
            let fence = try LiveRig(plan: try ReflexFixtures.clickPlan())
            fence.sleeper.onSleep = { index, _ in if index < 10 { fence.frame(.ball(track: 5, box: LiveRig.box)) } }
            fence.afterPost = { event in
                if case .buttonDown = event.kind { refusal.publish(on: fence) }
            }
            try fence.session.start()
            fence.frame(.ball(track: 5, box: LiveRig.box))
            _ = fence.receipts(1)
            XCTAssertEqual(fence.downs, 1, "\(refusal)")
            XCTAssertEqual(fence.ups, 1, "\(refusal): the button is let go of")
            XCTAssertEqual(fence.hand.snapshot.held, [])
            fence.session.stop(reason: StopReason.request)
        }
    }

    /// A newer capture that no longer shows the target — another track in its
    /// place, nothing there, nothing known — ends the glide: the proof from
    /// the capture before it is not carried past it.
    func test_a_newer_frame_without_the_target_ends_the_glide() throws {
        let sights: [(String, SceneKernel.Sight)] = [
            ("another track", .ball(track: 6, box: LiveRig.box)),
            ("gone", .absent),
            ("unknown", .unknown),
        ]
        for (name, sight) in sights {
            let rig = try LiveRig(plan: try ReflexFixtures.clickPlan())
            var eventsBefore = 0
            rig.sleeper.onSleep = { index, _ in
                switch index {
                case 0: rig.frame(.ball(track: 5, box: LiveRig.box))
                case 1:
                    eventsBefore = rig.poster.events.count
                    rig.frame(sight)
                default: break
                }
            }
            try rig.session.start()
            rig.frame(.ball(track: 5, box: LiveRig.box))
            let receipt = rig.receipts(1)
            XCTAssertEqual(rig.poster.events.count, eventsBefore, "\(name): no waypoint after it")
            XCTAssertEqual(rig.downs, 0, name)
            XCTAssertEqual(receipt.first?.outcome, .moved, name)
            rig.session.stop(reason: StopReason.request)
        }
    }

    // MARK: R3 — a reflex stop lets go before it tears anything down

    /// A reflex click holds the button in its fence; the run's stop is called
    /// with the capture teardown and then the monitor's held shut. The button
    /// is let go of before either: a teardown that hangs does not hold the
    /// release, and nothing is pressed after the stop.
    func test_a_reflex_stop_lets_go_before_it_tears_down_capture_and_monitor() throws {
        let rig = try LiveRig(plan: try ReflexFixtures.clickPlan())
        let inFence = Gate()
        inFence.close()
        rig.sleeper.onSleep = { index, _ in
            if index < 10 {
                rig.frame(.ball(track: 5, box: LiveRig.box))
            } else if index == 10 {
                inFence.pass()
            }
        }
        try rig.session.start()
        rig.frame(.ball(track: 5, box: LiveRig.box))
        XCTAssertTrue(inFence.awaitArrival(), "the click is in its fence")
        XCTAssertEqual(rig.hand.snapshot.held, [.button(.left)])
        rig.frames.teardown.close()
        rig.monitor.teardown.close()
        let stopped = DispatchSemaphore(value: 0)
        Thread.detachNewThread {
            rig.session.stop(reason: StopReason.request)
            stopped.signal()
        }
        XCTAssertTrue(rig.frames.teardown.awaitArrival(), "the stop reached the capture teardown")
        XCTAssertEqual(rig.ups, 1, "let go of before the capture is torn down")
        rig.frames.teardown.open()
        XCTAssertTrue(rig.monitor.teardown.awaitArrival(), "the stop reached the monitor's")
        XCTAssertEqual(rig.ups, 1, "let go of before the monitor is torn down")
        rig.monitor.teardown.open()
        XCTAssertEqual(stopped.wait(timeout: .now() + 5), .success)
        inFence.open()
        _ = rig.receipts(1)
        XCTAssertEqual(rig.downs, 1, "nothing pressed after the stop")
        XCTAssertEqual(rig.ups, 1)
        XCTAssertEqual(rig.hand.snapshot.holder, nil)
    }

    // MARK: R2 — a display that changed is a capture the run refuses

    /// The eye's captures hold only while the display is what the eye opened
    /// on: a display that moves or rescales makes the next capture — or, with
    /// no new frame, the reader's next read — one the run refuses, so nothing
    /// more goes with the old point transform. A frame the stream says is not
    /// a capture (blank, suspended) is refused the same way.
    func test_a_display_that_moved_or_rescaled_gives_the_old_transform_no_input() throws {
        let opened = FakeEyeFeed.display
        let moved = EyeGeometry(displayId: opened.displayId, originX: -1_440, originY: 0, width: opened.width, height: opened.height,
                                scale: opened.scale, orientation: opened.orientation)
        let rescaled = EyeGeometry(displayId: opened.displayId, originX: opened.originX, originY: opened.originY, width: opened.width,
                                   height: opened.height, scale: 2, orientation: opened.orientation)
        let extent = ReflexPixelExtent(width: 800, height: 500)

        // The book the eye keeps: the transform read off the display at each capture.
        var book = ReflexCaptureBook(opened: opened, generation: 3)
        let ready = book.delivered(extent: extent, colorSpace: .srgb, dirty: true, repaintSeq: 1, capturedNs: 10, deliveredNs: 11, now: opened)
        XCTAssertEqual(ready.status, .ready)
        XCTAssertEqual(ready.pointTransform, ReflexPointTransform(origin_x: 0, origin_y: 0, points_per_pixel: ReflexScale(numerator: 1, denominator: 1)))
        XCTAssertEqual(book.delivered(extent: extent, colorSpace: .srgb, dirty: true, repaintSeq: 2, capturedNs: 20, deliveredNs: 21, now: moved).status,
                       .interrupted, "a display that moved: the next capture is refused")
        XCTAssertEqual(book.delivered(extent: extent, colorSpace: .srgb, dirty: true, repaintSeq: 3, capturedNs: 30, deliveredNs: 31, now: opened).status,
                       .interrupted, "and stays so: the next eye is another stream")
        var quiet = ReflexCaptureBook(opened: opened, generation: 4)
        XCTAssertNil(quiet.interrupted(capturedNs: 1, deliveredNs: 1), "nothing to take back before the first capture")
        quiet.delivered(extent: extent, colorSpace: .srgb, dirty: true, repaintSeq: 1, capturedNs: 10, deliveredNs: 11, now: opened)
        XCTAssertEqual(quiet.interrupted(capturedNs: 20, deliveredNs: 21)?.status, .interrupted, "a blank or suspended frame")
        var read = ReflexCaptureBook(opened: opened, generation: 5)
        read.delivered(extent: extent, colorSpace: .srgb, dirty: true, repaintSeq: 1, capturedNs: 10, deliveredNs: 11, now: opened)
        XCTAssertEqual(read.read(open: true, now: opened)?.captureSeq, 1, "the same display: the capture as it was")
        XCTAssertEqual(read.read(open: true, now: rescaled)?.status, .interrupted)
        XCTAssertEqual(read.read(open: true, now: rescaled)?.captureSeq, 2, "refused once, then read as it is")
        var closed = ReflexCaptureBook(opened: opened, generation: 6)
        closed.delivered(extent: extent, colorSpace: .srgb, dirty: true, repaintSeq: 1, capturedNs: 10, deliveredNs: 11, now: opened)
        XCTAssertEqual(closed.read(open: false, now: opened)?.status, .interrupted, "an eye no longer open")

        // A run on the eye's own reader: capture 1 decides a click, capture 2
        // lets its first waypoint go, then the display changes.
        for (name, change) in [("moved, a new frame", moved), ("rescaled, no new frame", rescaled)] {
            let feed = FakeEyeFeed()
            let reader = ReflexEyeReader(eye: feed, reader: "run")
            let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(), source: reader)
            var eventsBefore = 0
            rig.sleeper.onSleep = { index, _ in
                switch index {
                case 0:
                    rig.settle(feed.deliver(showing: .ball(track: 5, box: LiveRig.box), on: rig.kernel, capturedNs: rig.clock.nowNs()),
                               taken: feed.taken, poke: feed.poke)
                case 1:
                    eventsBefore = rig.poster.events.count
                    feed.move(to: change)
                    if change == moved {
                        rig.settle(feed.deliver(showing: .ball(track: 5, box: LiveRig.box), on: rig.kernel, capturedNs: rig.clock.nowNs()),
                                   taken: feed.taken, poke: feed.poke)
                    } else {
                        // No new frame: the run reads the eye again, twice.
                        let reads = feed.taken(0)
                        feed.poke()
                        _ = eventually { feed.taken(0) >= reads + 1 }
                        feed.poke()
                        _ = eventually { feed.taken(0) >= reads + 2 }
                    }
                default: break
                }
            }
            try rig.session.start()
            rig.settle(feed.deliver(showing: .ball(track: 5, box: LiveRig.box), on: rig.kernel, capturedNs: rig.clock.nowNs() - 1_000_000),
                       taken: feed.taken, poke: feed.poke)
            let receipt = rig.receipts(1)
            XCTAssertEqual(rig.poster.events.count, eventsBefore, "\(name): nothing goes with the old transform")
            XCTAssertEqual(rig.downs, 0, name)
            XCTAssertEqual(receipt.first?.outcome.rawValue, "evidence", name)
            rig.session.stop(reason: StopReason.request)
        }
    }

    // MARK: R3 — a stop during a start cancels the start

    /// A reflexStop that comes while a run is still starting — its monitor
    /// proving itself, or its kernel preparing, held there — ends that start:
    /// the stop names the run, the start publishes nothing and gives the hand
    /// back, no run stands and nothing is posted after the stop.
    func test_a_reflex_stop_during_start_cancels_the_start() throws {
        let saved = ReflexRuntimeHost.parts
        defer {
            ReflexRuntimeHost.parts = saved
            ReflexRuntimeHost.kernel = nil
        }
        let fixtures = ReflexFixtures.root
        let limits = Data(try Data(contentsOf: fixtures.appendingPathComponent("reflex-contract/limits.json")).dropLast())
        let perception = Data(try Data(contentsOf: fixtures.appendingPathComponent("game-state/limits.json")).dropLast())
        let wire = try ReflexFixtures.wire()
        let eye = try XCTUnwrap(EyeConfig(framesPerSecond: 30, changesKept: 512, idleStopMs: 30_000, firstFrameMs: 2_000,
                                          ocrCellPoints: 24, ocrMarginPoints: 4, ocrMaxShare: 0.5, ocrMaxPieces: 4))
        for held in ["the kernel preparing", "the monitor proving itself"] {
            let kernel = GatedKernel()
            let monitor = SyncEchoMonitor()
            let frames = SteppedFrames()
            let gate = held == "the kernel preparing" ? kernel.gate : monitor.starting
            gate.close()
            ReflexRuntimeHost.kernel = kernel
            ReflexRuntimeHost.parts = .init(reader: { _, _, _, _ in frames }, monitor: { _ in monitor }, boundary: { _ in PermitEverywhere() })
            let clock = FakeClock(DispatchTime.now().uptimeNanoseconds)
            let poster = RecordingPoster()
            let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 0x57a7)
            let me = Int64(getpid())
            poster.onPost = { _, tag in monitor.hear(InputOrigin.of(userData: tag, sourcePid: me, handTag: hand.tag, handPid: me)) }
            let outcome = Box<String>()
            Thread.detachNewThread {
                do {
                    _ = try ReflexRuntimeHost.start(
                        runId: "racing", plan: wire, limits: limits, perception: perception, eye: eye, display: 0,
                        hand: hand, admit: { .admitted },
                        actingScope: { ReflexActingScope(surface: $0.surface, target: $0.target, pid: 4_242) }
                    )
                    outcome.value = "published"
                } catch let error as ProviderError {
                    outcome.value = error.code
                } catch {
                    outcome.value = "\(error)"
                }
            }
            XCTAssertTrue(gate.awaitArrival(), "\(held): the start is held")
            let stopped = ReflexRuntimeHost.stop(reason: StopReason.request)
            XCTAssertEqual(stopped["state"] as? String, "stopped", "\(held): the stop found the starting run")
            XCTAssertEqual(stopped["runId"] as? String, "racing", held)
            let postedAtStop = poster.events.count
            gate.open()
            XCTAssertTrue(eventually { outcome.value != nil }, held)
            XCTAssertEqual(outcome.value, "stopped", "\(held): the start ended without publishing")
            XCTAssertEqual(ReflexRuntimeHost.status()["state"] as? String, "none", "\(held): no run stands")
            XCTAssertNil(hand.snapshot.holder, "\(held): the hand was given back")
            XCTAssertEqual(poster.events.count, postedAtStop, "\(held): nothing posted after the stop")
            XCTAssertEqual(poster.presses.count, 0, held)
            _ = ReflexRuntimeHost.stop(reason: StopReason.request)
        }
    }

    // MARK: R4 — a reflex press passes the verbs' guard and the run's target

    /// The boundary a helper's run carries (`DesktopRunBoundary`), with the
    /// window server's answer played by the test: a ball on a window of the
    /// app the run acts in is pressed; on ZeroCode's own window, another
    /// app's or nothing at all nothing is posted; and ZeroCode's window coming
    /// over the ball during the glide stops the press.
    func test_a_reflex_press_passes_the_self_window_and_target_guard() throws {
        let scope = ReflexActingScope(surface: .macos_desktop, target: "fixture", pid: 4_242)
        let zeroCode: pid_t = 7
        func run(_ owner: @escaping @Sendable (CGPoint) -> pid_t?, prepare: (LiveRig) -> Void = { _ in }) throws -> (LiveRig, ReflexReceipt?, Int) {
            let boundary = DesktopRunBoundary(scope: scope, owner: owner, isOwn: { $0 == zeroCode })
            let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(), boundary: boundary)
            rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: 5, box: LiveRig.box)) }
            prepare(rig)
            try rig.session.start()
            let echoed = rig.poster.events.count
            rig.frame(.ball(track: 5, box: LiveRig.box))
            let receipt = rig.receipts(1).first
            rig.session.stop(reason: StopReason.request)
            return (rig, receipt, rig.poster.events.count - echoed)
        }
        let (target, pressed, _) = try run { _ in 4_242 }
        XCTAssertEqual(pressed?.outcome, .done, "a window of the run's app")
        XCTAssertEqual(target.downs, 1)
        XCTAssertEqual(target.ups, 1)

        for (name, owner) in [("ZeroCode's own window", zeroCode), ("another app's window", pid_t(99))] {
            let (rig, refused, posted) = try run { _ in owner }
            XCTAssertEqual(refused?.outcome.rawValue, "scope", name)
            XCTAssertEqual(posted, 0, "\(name): nothing posted")
            XCTAssertEqual(rig.downs, 0, name)
        }
        let (_, nowhere, nothingPosted) = try run { _ in nil }
        XCTAssertEqual(nowhere?.outcome.rawValue, "scope", "no window under the point")
        XCTAssertEqual(nothingPosted, 0)

        let under = Box<pid_t>()
        under.value = 4_242
        let (covered, late, _) = try run({ _ in under.value }) { rig in
            var moves = 0
            rig.afterPost = { event in
                guard event.kind == .pointerMove else { return }
                moves += 1
                // The echo, then the glide's ten: ZeroCode's window comes over before the press.
                if moves == 11 { under.value = zeroCode }
            }
        }
        XCTAssertEqual(late?.outcome.rawValue, "scope", "covered before the press")
        XCTAssertEqual(covered.downs, 0)
    }

    /// The ask before the press takes its time (the window server answers
    /// it), and what became known meanwhile is read before the press:
    /// evidence taken back, the clock past the frame's age and the lease's
    /// end, a newer capture with the target where the pointer is not, the
    /// pointer off the point asked — none of them presses. Allowed at once, or on a newer capture that still
    /// holds the pointer, the press goes where the boundary was asked.
    func test_what_became_known_while_the_boundary_answered_is_read_before_the_press() throws {
        let limits = try ReflexFixtures.limits()
        func run(duringTheAsk step: @escaping (LiveRig) -> Void) throws -> (LiveRig, ReflexReceipt?, AskedBoundary) {
            let boundary = AskedBoundary()
            let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(), boundary: boundary)
            rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: 5, box: LiveRig.box)) }
            // The leaf asks where it goes, then where it presses.
            boundary.during = { ask in if ask == 2 { step(rig) } }
            try rig.session.start()
            rig.frame(.ball(track: 5, box: LiveRig.box))
            let receipt = rig.receipts(1).first
            rig.session.stop(reason: StopReason.request)
            return (rig, receipt, boundary)
        }
        func pressedWhereAsked(_ rig: LiveRig, _ boundary: AskedBoundary, _ name: String) {
            XCTAssertEqual(boundary.points.count, 2, "\(name): asked where it goes and where it presses, once each")
            XCTAssertEqual(rig.poster.presses.map { SmoothPointerPath.Point(x: $0.x, y: $0.y) }, [boundary.points[1]],
                           "\(name): pressed where the boundary was asked")
            XCTAssertEqual(rig.downs, 1, name)
            XCTAssertEqual(rig.ups, 1, "\(name): let go of")
        }

        let (allowed, done, asked) = try run { _ in }
        XCTAssertEqual(done?.outcome, .done, "allowed at once")
        pressedWhereAsked(allowed, asked, "allowed at once")

        // A newer capture refused while the boundary answered: the evidence is taken back first.
        let taken = Box<(latest: Bool, refused: UInt64)>()
        let (refused, evidence, _) = try run { rig in
            let before = rig.session.status.framesRefused
            rig.frame(.ball(track: 5, box: LiveRig.box), status: .interrupted)
            taken.value = (rig.session.sightings.latest() != nil, rig.session.status.framesRefused - before)
        }
        XCTAssertEqual(taken.value?.latest, false, "the refusal took the evidence back before the boundary answered")
        XCTAssertEqual(taken.value?.refused, 1)
        XCTAssertEqual(evidence?.outcome, .evidence)
        XCTAssertEqual(refused.downs, 0, "no press on evidence taken back")
        XCTAssertEqual(refused.hand.snapshot.held, [])

        // The clock past the frame's age and the lease's end while the boundary answered.
        let (late, ended, _) = try run { rig in rig.clock.set(rig.clock.nowNs() + limits.max_frame_age_ns + limits.max_lease_ns + 1) }
        XCTAssertEqual(ended?.outcome, .lease)
        XCTAssertEqual(late.downs, 0, "no press on a lease that ended")

        // A newer capture with the target where the pointer is not.
        let far = ReflexRoi(x: 0, y: 0, width: 4, height: 4, space: .pixel)
        let (away, left, _) = try run { rig in rig.frame(.ball(track: 5, box: far)) }
        XCTAssertEqual(left?.outcome, .moved)
        XCTAssertEqual(away.downs, 0, "no press where the target no longer is")

        // The pointer moved off the point asked, before any monitor heard it.
        let (drifted, off, _) = try run { rig in
            guard let asked = rig.hand.pointerNow() else { return }
            rig.poster.movePointer(to: SmoothPointerPath.Point(x: asked.x + 1, y: asked.y))
        }
        XCTAssertEqual(off?.outcome, .moved)
        XCTAssertEqual(drifted.downs, 0, "no press on a point the boundary was not asked about")

        // A newer capture that still holds the pointer: pressed where the boundary was asked.
        let (held, still, heldAsked) = try run { rig in rig.frame(.ball(track: 5, box: LiveRig.box)) }
        XCTAssertEqual(still?.outcome, .done, "a newer capture that still holds the pointer")
        pressedWhereAsked(held, heldAsked, "a newer capture that still holds the pointer")
    }

    /// The helper binds a run to its plan's own target when it starts: a
    /// target no running app answers to is refused before the eye or the
    /// hand, and one that resolves hands the run's boundary that app — the
    /// plan's scope, never ZeroCode itself (`Provider.actingScope`).
    func test_a_reflex_start_binds_the_plans_target_before_the_eye_or_the_hand() throws {
        let saved = ReflexRuntimeHost.parts
        defer {
            ReflexRuntimeHost.parts = saved
            ReflexRuntimeHost.kernel = nil
        }
        let fixtures = ReflexFixtures.root
        let limits = String(decoding: try Data(contentsOf: fixtures.appendingPathComponent("reflex-contract/limits.json")).dropLast(), as: UTF8.self)
        let perception = String(decoding: try Data(contentsOf: fixtures.appendingPathComponent("game-state/limits.json")).dropLast(), as: UTF8.self)
        let eye: JSONValue = .object([
            "framesPerSecond": .number(30), "changesKept": .number(512), "idleStopMs": .number(30_000), "firstFrameMs": .number(2_000),
            "ocrCellPoints": .number(24), "ocrMarginPoints": .number(4), "ocrMaxShare": .number(0.5), "ocrMaxPieces": .number(4),
        ])
        struct Stop: Error {}
        let bound = Box<ReflexActingScope>()
        let readers = Box<Int>()
        ReflexRuntimeHost.kernel = SceneKernel()
        ReflexRuntimeHost.parts = .init(
            reader: { _, _, _, _ in
                readers.value = (readers.value ?? 0) + 1
                throw Stop()
            },
            monitor: { _ in SyncEchoMonitor() },
            boundary: { scope in
                bound.value = scope
                return PermitEverywhere()
            }
        )
        func start(target: String, surface: String = "macos_desktop") -> String? {
            do {
                let wire = try ReflexFixtures.wire { $0["scope"] = ["surface": surface, "target": target] }
                _ = try Provider().handle(method: "reflexStart", params: [
                    "runId": .string("binding"), "plan": .string(String(decoding: wire, as: UTF8.self)), "limits": .string(limits),
                    "perception": .string(perception), "eye": eye, "display": .number(0),
                ])
                return nil
            } catch {
                return (error as? ProviderError)?.code ?? "\(error)"
            }
        }
        let posted = OperatorHandHost.hand.snapshot.posted
        XCTAssertEqual(start(target: "no-such-app-anywhere"), "app_not_found", "a target no running app answers to")
        XCTAssertNil(readers.value, "refused before the eye")
        XCTAssertEqual(start(target: "fixture", surface: "ios_device"), "unsupported_capability", "another surface's plan")
        XCTAssertNil(readers.value)
        let finder = try XCTUnwrap(NSWorkspace.shared.runningApplications.first { $0.bundleIdentifier == "com.apple.finder" },
                                   "Finder runs in every desktop session")
        _ = start(target: "com.apple.finder")
        XCTAssertEqual(bound.value, ReflexActingScope(surface: .macos_desktop, target: "com.apple.finder", pid: finder.processIdentifier),
                       "the run's boundary holds the app its plan names")
        XCTAssertEqual(readers.value, 1)
        XCTAssertEqual(OperatorHandHost.hand.snapshot.holder, nil, "the hand was never taken")
        XCTAssertEqual(OperatorHandHost.hand.snapshot.posted, posted, "nothing posted")
        XCTAssertEqual(ReflexRuntimeHost.status()["state"] as? String, "none")
    }
}

/// An eye as its reader sees it (`EyeFeed`), fed by the test: its capture
/// book is the helper's own, the display it stands on is the test's to move,
/// and the pixels are one BGRA buffer of the fixture's extent.
final class FakeEyeFeed: EyeFeed, @unchecked Sendable {
    static let display = EyeGeometry(displayId: "fixture", originX: 0, originY: 0, width: 800, height: 500, scale: 1, orientation: .up)

    private let lock = NSLock()
    private var book = ReflexCaptureBook(opened: FakeEyeFeed.display, generation: 1)
    private var standing = FakeEyeFeed.display
    private var wakes: [String: @Sendable () -> Void] = [:]
    private var reads: [UInt64] = []
    private let pixels: CVPixelBuffer

    init() {
        var buffer: CVPixelBuffer?
        CVPixelBufferCreate(kCFAllocatorDefault, 800, 500, kCVPixelFormatType_32BGRA, nil, &buffer)
        pixels = buffer!
    }

    var nextSeq: UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return book.captureSeq + 1
    }

    /// The display now stands at `geometry`.
    func move(to geometry: EyeGeometry) {
        lock.lock()
        standing = geometry
        lock.unlock()
    }

    /// The stream delivers a complete frame captured at `capturedNs` that the
    /// kernel sees `sight` on; answers its capture number.
    func deliver(showing sight: SceneKernel.Sight, on kernel: SceneKernel, capturedNs: UInt64) -> UInt64 {
        lock.lock()
        let seq = book.captureSeq + 1
        kernel.show(sight, on: seq)
        book.delivered(extent: ReflexPixelExtent(width: 800, height: 500), colorSpace: .srgb, dirty: true, repaintSeq: seq,
                       capturedNs: capturedNs, deliveredNs: capturedNs, now: standing)
        let woken = Array(wakes.values)
        lock.unlock()
        for wake in woken { wake() }
        return seq
    }

    func poke() {
        lock.lock()
        let woken = Array(wakes.values)
        lock.unlock()
        for wake in woken { wake() }
    }

    /// How many of the run's reads took capture `seq` or a newer one.
    func taken(_ seq: UInt64) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return reads.filter { $0 >= seq }.count
    }

    func readerCapture() -> (pixels: CVPixelBuffer?, capture: ReflexCapture)? {
        lock.lock()
        defer { lock.unlock() }
        guard let capture = book.read(open: true, now: standing) else { return nil }
        reads.append(capture.captureSeq)
        return (pixels, capture)
    }

    func wake(_ reader: String, _ wake: (@Sendable () -> Void)?) {
        lock.lock()
        wakes[reader] = wake
        lock.unlock()
    }

    func remove(_ reader: String) {
        lock.lock()
        wakes[reader] = nil
        lock.unlock()
    }
}

/// A kernel whose session waits at `gate` while it prepares, then answers the scene.
final class GatedKernel: ReflexPerceptionKernel, @unchecked Sendable {
    let gate = Gate()
    let scene = SceneKernel()

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        gate.pass()
        return try scene.session(for: plan, limits: limits)
    }
}

extension SteppedFrames: ReflexRunSource {
    func end() {}
}
