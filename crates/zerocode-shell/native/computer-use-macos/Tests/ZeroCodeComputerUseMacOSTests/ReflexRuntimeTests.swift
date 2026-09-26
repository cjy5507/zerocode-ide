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

    /// The window's tables as a start carries them: the files' own bytes.
    static func limitsWire() throws -> Data { Data(try Data(contentsOf: root.appendingPathComponent("reflex-contract/limits.json")).dropLast()) }
    static func perceptionWire() throws -> Data { Data(try Data(contentsOf: root.appendingPathComponent("game-state/limits.json")).dropLast()) }

    /// A run policy as the window writes it (`RunPolicy::wire`).
    static func policy(renew: Bool = false, runNs: UInt64? = nil) throws -> ReflexRunPolicy {
        ReflexRunPolicy(version: ReflexContract.runPolicyVersion, run_ns: try runNs ?? limits().max_run_ns, renew: renew)
    }

    static func policyWire(renew: Bool = false, runNs: UInt64? = nil) throws -> Data {
        try JSONSerialization.data(withJSONObject: try JSONSerialization.jsonObject(with: JSONEncoder().encode(policy(renew: renew, runNs: runNs))),
                                   options: [.sortedKeys, .withoutEscapingSlashes])
    }

    /// The capability table's own bytes with the desktop's live reflex set to
    /// `liveReflex` — either way, never read off the file's row, which is the
    /// window's to turn on and off.
    static func capabilityWire(liveReflex: Bool) throws -> Data {
        let raw = Data(try Data(contentsOf: root.appendingPathComponent("reflex-contract/capability.json")).dropLast())
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: raw) as? [String: Any])
        var surfaces = try XCTUnwrap(object["surfaces"] as? [String: Any])
        surfaces["macos_desktop"] = ["instant_pointer": false, "live_reflex": liveReflex]
        object["surfaces"] = surfaces
        return try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])
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
    static func frame(capture: UInt64, capturedNs: UInt64, deliveredNs: UInt64? = nil, owner: UInt64, stream: UInt64 = 1,
                      run: String = "run") -> ReflexFrameFacts {
        self.capture(capture, capturedNs: capturedNs, deliveredNs: deliveredNs, stream: stream).facts(runId: run, ownerEpoch: owner, planEpoch: 1)
    }

    /// Capture `seq` of the 800 × 500 fixture extent, one point a pixel, as a
    /// platform adapter writes it: ready unless `status` says otherwise, its
    /// capture time unknown when `capturedNs` is nil, and delivered when it
    /// was captured unless `deliveredNs` says otherwise.
    static func capture(_ seq: UInt64, capturedNs: UInt64?, deliveredNs: UInt64? = nil, status: ReflexFrameStatus = .ready,
                        stream: UInt64 = 1) -> ReflexCapture {
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
            deliveredHostNs: deliveredNs ?? capturedNs,
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
    /// `maxFires` times, the ball's target picked by `pick`.
    static func clickPlan(maxFires: Int = 1, pick: ReflexPick = .first) throws -> ValidatedReflexPlan {
        try plan { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [["id": "click1", "kind": "click", "target": "ball"]]]]
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["max_fires"] = maxFires
            rules[0]["cooldown_ms"] = 0
            rules[0]["predicate"] = ["op": "eq", "value": 1]
            object["rules"] = rules
            if pick != .first {
                var detectors = object["detectors"] as! [[String: Any]]
                detectors[0]["pick"] = pick.rawValue
                object["detectors"] = detectors
            }
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

    /// The frame the leaf is decided on: capture 10, delivered a millisecond
    /// ago and stamped `ahead` after its delivery.
    func decide(ahead: UInt64 = 0) {
        let delivered = clock.nowNs() - 1_000_000
        let source = ReflexFixtures.frame(capture: 10, capturedNs: delivered + ahead, deliveredNs: delivered, owner: token.id)
        sightings.publish(ReflexFixtures.seen(source, ReflexFixtures.ball(on: source, track: 5, box: box)))
    }

    /// A newer capture, just delivered and stamped `ahead` after its delivery,
    /// showing `sighting` (or the ball where it was).
    func capture(_ seq: UInt64, track: UInt64 = 5, box moved: ReflexRoi? = nil, ahead: UInt64 = 0) {
        let frame = ReflexFixtures.frame(capture: seq, capturedNs: clock.nowNs() + ahead, deliveredNs: clock.nowNs(), owner: token.id)
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
            boundary: PermitEverywhere(),
            deadlineNs: .max
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

    /// ScreenCaptureKit stamps a frame with the time the display shows it,
    /// which runs ahead of the frame's delivery (t-10127). Every frame here —
    /// the one the leaf is decided on and each newer one, read the moment it
    /// is delivered — is stamped 3 ms (within a frame) or 30 ms (past one)
    /// after its delivery: the leaf aims without carrying the target forward
    /// or back, its lease holds from each delivery, and it presses. Before,
    /// the first aim ended it `unaimed`. The receipt keeps both stamps of
    /// each frame, so the lead shows.
    func test_frames_stamped_ahead_of_their_delivery_are_read_from_their_delivery() throws {
        for ahead: UInt64 in [3_000_000, 30_000_000] {
            let rig = try LeafRig()
            rig.decide(ahead: ahead)
            var seq: UInt64 = 10
            rig.sleeper.onSleep = { _, _ in
                seq += 1
                rig.capture(seq, ahead: ahead)
            }
            let done = try rig.runner().run(LeafRig.click, index: 0)
            XCTAssertEqual(done.outcome, .done, "\(ahead) ns ahead")
            XCTAssertEqual(done.events, 12, "ten waypoints, a press and its release")
            XCTAssertEqual(rig.poster.presses.map(\.kind), [.buttonDown(.left, clickState: 1)])
            XCTAssertEqual(rig.poster.releases.map(\.kind), [.buttonUp(.left, clickState: 1)])
            let decided = try XCTUnwrap(done.decidedHostNs), decidedDelivered = try XCTUnwrap(done.decidedDeliveredHostNs)
            XCTAssertEqual(decided - decidedDelivered, ahead, "the decided frame's two stamps")
            let first = try XCTUnwrap(done.firstEventFrameHostNs), firstDelivered = try XCTUnwrap(done.firstEventFrameDeliveredHostNs)
            XCTAssertEqual(first - firstDelivered, ahead, "the permitting frame's two stamps")
            XCTAssertLessThan(firstDelivered, first, "delivered before the display time it carries")
        }
    }

    /// A lease's target proof runs `max_frame_age_ns` from when its frame was
    /// in hand — its delivery, for a frame stamped ahead of it — never from a
    /// display time that stands after the delivery, and a renewal by such a
    /// frame the same (t-10127).
    func test_a_lease_proves_its_target_from_when_its_frame_was_in_hand() throws {
        let rig = try LeafRig()
        let delivered = rig.clock.nowNs()
        let ahead: UInt64 = 30_000_000
        let frame = ReflexFixtures.frame(capture: 10, capturedNs: delivered + ahead, deliveredNs: delivered, owner: rig.token.id)
        let target = try XCTUnwrap(ReflexFixtures.ball(on: frame, track: 5, box: rig.box).target)
        let lease = ReflexActionLease.issue(runId: "run", leaf: LeafRig.click, target: target, frame: frame, nowNs: delivered,
                                            deadlineNs: .max, children: 12, limits: rig.limits)
        XCTAssertEqual(lease.target_proof_until_host_ns, delivered + rig.limits.max_frame_age_ns)
        let newer = ReflexFixtures.frame(capture: 11, capturedNs: delivered + 2 * ahead, deliveredNs: delivered + ahead, owner: rig.token.id)
        XCTAssertEqual(lease.renewed(by: newer, target: target, limits: rig.limits).target_proof_until_host_ns,
                       delivered + ahead + rig.limits.max_frame_age_ns)
        // A frame captured before its delivery keeps its capture time.
        let early = ReflexFixtures.frame(capture: 12, capturedNs: delivered, deliveredNs: delivered + ahead, owner: rig.token.id)
        XCTAssertEqual(lease.renewed(by: early, target: target, limits: rig.limits).target_proof_until_host_ns,
                       delivered + rig.limits.max_frame_age_ns)
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
        var book = ReflexRuleBook(plan, policy: try ReflexFixtures.policy())
        func read(_ capture: UInt64, _ value: Int64?, at now: UInt64 = 0, free: Bool = true) -> String? {
            book.read(streamEpoch: 1, captureSeq: capture, values: value.map { ["ball": $0] } ?? [:], nowNs: now, handFree: free).fired?.id
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
        XCTAssertNil(book.read(streamEpoch: 1, captureSeq: 2, values: ["ball": 1], nowNs: 0, handFree: true).fired, "an older capture is not new")
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
    /// fills the queue, and the next leaf ends the run's acting instead of
    /// blocking on the reader; the hand still lets go of what it holds.
    func test_slow_evidence_consumer_cannot_block_the_hand() throws {
        let receipts = ReflexReceipts(capacity: 2)
        let consumerBusy = DispatchSemaphore(value: 0)
        let consumerRelease = DispatchSemaphore(value: 0)
        Thread.detachNewThread {
            _ = receipts.read(after: 0, limit: 0)
            consumerBusy.signal()
            _ = consumerRelease.wait(timeout: .now() + 10) // the reader is stuck elsewhere
        }
        XCTAssertEqual(consumerBusy.wait(timeout: .now() + 5), .success)
        let leaves = (0..<5).map { ReflexLeaf(ruleId: "follow", actionId: "a\($0)", kind: .move, detector: "ball") }
        var next: UInt64 = 0
        var ran: [String] = []
        var overflowed = 0
        let started = Date()
        let count = ReflexSession.runFire(leaves, receipts: receipts, acting: { true }, overflowed: { overflowed += 1 }, next: &next) { leaf, index in
            ran.append(leaf.actionId)
            return ReflexReceipt(ruleId: leaf.ruleId, actionId: leaf.actionId, leafIndex: index, outcome: .done, targetId: nil,
                                 trackId: nil, pick: .first, sourceCapture: nil, decidedHostNs: nil, decidedDeliveredHostNs: nil, admittedHostNs: nil, captureWaitNs: 0,
                                 firstEventHostNs: nil, firstEventFrameHostNs: nil, firstEventFrameDeliveredHostNs: nil,
                                 downHostNs: nil, upHostNs: nil, endedHostNs: 0, events: 1)
        }
        XCTAssertLessThan(Date().timeIntervalSince(started), 1, "never waited on the reader")
        XCTAssertEqual(count, 2)
        XCTAssertEqual(ran, ["a0", "a1"], "the third leaf never starts, and nothing blocks")
        XCTAssertEqual(overflowed, 1, "a full queue ends the fire and says so once")
        XCTAssertEqual(receipts.pending, 2)
        // The hand is the hand's: a full queue does not stand between a stop and the release.
        let rig = try LeafRig()
        try rig.hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 3, y: 3), by: rig.token)
        XCTAssertEqual(rig.hand.stop(reason: StopReason.hotkey).released, [.button(.left)])
        consumerRelease.signal()
        XCTAssertEqual(receipts.read(after: 0, limit: 10).map(\.receipt.actionId), ["a0", "a1"])
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
        let policy = String(decoding: try ReflexFixtures.policyWire(), as: UTF8.self)
        let claimed = String(decoding: try ReflexFixtures.capabilityWire(liveReflex: true), as: UTF8.self)
        let table = String(decoding: try ReflexFixtures.capabilityWire(liveReflex: false), as: UTF8.self)
        // The window's own table, as it sends it: the file's canonical bytes.
        let golden = String(decoding: Data(try Data(contentsOf: fixtures.appendingPathComponent("reflex-contract/capability.json")).dropLast()), as: UTF8.self)
        var said = ""
        func start(_ change: (inout [String: JSONValue]) -> Void) -> String? {
            var params: [String: JSONValue] = [
                "runId": .string("wiring"), "plan": .string(wire), "limits": .string(limits),
                "perception": .string(perception), "runPolicy": .string(policy), "capability": .string(claimed),
                "eye": eye, "display": .number(0),
            ]
            change(&params)
            do {
                _ = try Provider().handle(method: "reflexStart", params: params)
                return nil
            } catch let error as ProviderError {
                said = error.message
                return error.code
            } catch {
                return "\(error)"
            }
        }
        XCTAssertNil(ReflexRuntimeHost.kernel, "the test process never launched the helper")
        XCTAssertEqual(start { $0["capability"] = .string(golden) }, "unsupported_capability")
        XCTAssertTrue(said.contains("no perception kernel"),
                      "the window's own table claims the desktop; what this helper lacks is its kernel: \(said)")
        XCTAssertEqual(start { _ in }, "unsupported_capability", "a helper with no kernel installed: refused before the eye or the hand")
        XCTAssertTrue(said.contains("no perception kernel"), said)
        XCTAssertEqual(start { $0["capability"] = .string(table) }, "unsupported_capability",
                       "a table that claims no live reflex on the desktop: refused before the eye or the hand")
        XCTAssertTrue(said.contains("claims no live reflex"), "refused for the table, before a kernel is looked for: \(said)")
        XCTAssertEqual(start { $0["capability"] = nil }, "invalid_argument", "no capability table, no run")
        XCTAssertEqual(start { $0["capability"] = .string(" " + claimed) }, "invalid_argument", "only the table's canonical bytes")
        XCTAssertEqual(start { $0["runPolicy"] = nil }, "invalid_argument", "no run policy, no run")
        XCTAssertEqual(start { $0["runPolicy"] = .string(policy.replacingOccurrences(of: "\"version\":1", with: "\"version\":2")) }, "invalid_argument",
                       "a later run policy is refused, never read as this one")
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
        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                     budget: inout ReflexPerceptionBudget) -> [ReflexObservation] { [] }
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
                                             perception: try ReflexFixtures.perception(), planEpoch: 1,
                                             policy: try ReflexFixtures.policy(), deadlineNs: .max),
            hand: hand, source: ScriptedFrames(), kernel: SilentKernel(), monitor: monitor,
            admit: { .admitted }, standing: { .admitted },
            fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
            boundary: PermitEverywhere(), alarm: ManualAlarm()
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

        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                     budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
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

    private var made = 0

    /// How many sessions this kernel opened.
    var sessions: Int {
        lock.lock()
        defer { lock.unlock() }
        return made
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        lock.lock()
        made += 1
        lock.unlock()
        return Session(kernel: self)
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
    private var interrupted: (@Sendable (String) -> Void)?

    func start(heard: @escaping @Sendable (InputOrigin) -> Void, interrupted: @escaping @Sendable (String) -> Void) throws {
        starting.pass()
        lock.lock()
        self.heard = heard
        self.interrupted = interrupted
        lock.unlock()
    }

    func stop() {
        teardown.pass()
        lock.lock()
        heard = nil
        interrupted = nil
        lock.unlock()
    }

    /// The system turned the tap off and on again.
    func interrupt(_ reason: String) {
        lock.lock()
        let interrupted = self.interrupted
        lock.unlock()
        interrupted?(reason)
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
    /// The scene `frame` shows its sights on — what the run's kernel reads,
    /// directly or through a kernel wrapping it.
    let kernel: SceneKernel
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

    /// A run of `plan` under `policy` reading `frames`, or `source` when one is
    /// given, with the kernel `kernel` when one is given; its presses held to
    /// `boundary`, its leaves admitted and its renewals read by `ledger`, its
    /// deadline `runNs` after it began — on `alarm`, which only the test rings.
    init(plan: ValidatedReflexPlan, policy: ReflexRunPolicy? = nil, boundary: any ReflexInputBoundary = PermitEverywhere(),
         source: (any ReflexFrameSource)? = nil, scene: SceneKernel = SceneKernel(), kernel: (any ReflexPerceptionKernel)? = nil,
         limits: ReflexLimits? = nil, ledger: TestLedger = TestLedger(), runNs: UInt64? = nil) throws {
        self.kernel = scene
        sleeper = ScriptedSleeper(clock: clock)
        hand = OperatorHand(poster: poster, clock: clock, sleeper: sleeper, tag: 0x5e55)
        self.ledger = ledger
        let limits = try limits ?? ReflexFixtures.limits()
        let policy = try policy ?? ReflexFixtures.policy()
        deadlineNs = clock.nowNs() + (runNs ?? policy.run_ns)
        let hand = self.hand
        let monitor = self.monitor
        let me = Int64(getpid())
        poster.onPost = { [weak self] event, tag in
            monitor.hear(InputOrigin.of(userData: tag, sourcePid: me, handTag: hand.tag, handPid: me))
            self?.afterPost?(event)
        }
        session = ReflexSession(
            settings: ReflexSession.Settings(runId: "run", plan: plan, limits: limits, perception: try ReflexFixtures.perception(), planEpoch: 1,
                                             policy: policy, deadlineNs: deadlineNs),
            hand: hand, source: source ?? frames, kernel: kernel ?? self.kernel, monitor: monitor,
            admit: { ledger.admit() }, standing: { ledger.standing() },
            fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
            boundary: boundary, alarm: alarm
        )
    }

    let alarm = ManualAlarm()
    let ledger: TestLedger
    let deadlineNs: UInt64
    private let trackLock = NSLock()
    private var shownTrack: UInt64 = 5
    /// The track the ball is shown on — set by the test, read by the leaf's steps.
    var track: UInt64 {
        get {
            trackLock.lock()
            defer { trackLock.unlock() }
            return shownTrack
        }
        set {
            trackLock.lock()
            shownTrack = newValue
            trackLock.unlock()
        }
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
        let seq = publish(sight, status: status, captured: captured)
        settle(seq, taken: frames.taken, poke: frames.poke)
        return seq
    }

    /// Publish the next capture showing `sight` without waiting for the run to
    /// read it — for a run that may have ended on the capture before.
    @discardableResult
    func publish(_ sight: SceneKernel.Sight, status: ReflexFrameStatus = .ready, captured: Captured = .now) -> UInt64 {
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
        return seq
    }

    /// The leaves the run finished, their receipts read and acknowledged in
    /// order, as the window's collector does.
    func receipts(_ count: Int, seconds: Double = 5) -> [ReflexReceipt] {
        var taken: [ReflexReceipts.Entry] = []
        _ = eventually(within: seconds) {
            let fresh = session.receipts.read(after: taken.last?.seq ?? 0, limit: count - taken.count)
            taken += fresh
            if let last = fresh.last { session.receipts.acknowledge(through: last.seq) }
            return taken.count >= count
        }
        return taken.map(\.receipt)
    }

    var downs: Int { poster.presses.filter { if case .buttonDown = $0.kind { return true } else { return false } }.count }
    var ups: Int { poster.releases.filter { if case .buttonUp = $0.kind { return true } else { return false } }.count }

    /// The ball goes and comes back on `track` — a new edge — while the pointer
    /// has been moved off it (by nobody the monitor heard), so the next leaf
    /// glides: a leaf that starts on its target waits for a capture the scripted
    /// sleeper only publishes at its deadline.
    func awayAndBack(status: ReflexFrameStatus = .ready, waiting: Bool = true) {
        poster.movePointer(to: SmoothPointerPath.Point(x: 0, y: 0))
        if waiting {
            frame(.absent, status: status)
            frame(.ball(track: track, box: LiveRig.box), status: status)
        } else {
            let away = publish(.absent, status: status)
            _ = eventually(within: 1) { frames.taken(away) >= 1 }
            publish(.ball(track: track, box: LiveRig.box), status: status)
        }
    }
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
        let wire = try ReflexFixtures.wire()
        for (held, runId) in [("the kernel preparing", "racing1"), ("the monitor proving itself", "racing2")] {
            let kernel = GatedKernel()
            let monitor = SyncEchoMonitor()
            let frames = SteppedFrames()
            let gate = held == "the kernel preparing" ? kernel.gate : monitor.starting
            gate.close()
            ReflexRuntimeHost.kernel = kernel
            ReflexRuntimeHost.parts = .init(reader: { _, _, _, _ in frames }, monitor: { _ in monitor }, boundary: { _ in PermitEverywhere() },
                                            alarm: { ManualAlarm() })
            let clock = FakeClock(DispatchTime.now().uptimeNanoseconds)
            let poster = RecordingPoster()
            let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 0x57a7)
            let me = Int64(getpid())
            poster.onPost = { _, tag in monitor.hear(InputOrigin.of(userData: tag, sourcePid: me, handTag: hand.tag, handPid: me)) }
            let outcome = Box<String>()
            Thread.detachNewThread {
                do {
                    _ = try HostStart(runId: runId, plan: wire, hand: hand).start()
                    outcome.value = "published"
                } catch let error as ProviderError {
                    outcome.value = error.code
                } catch {
                    outcome.value = "\(error)"
                }
            }
            XCTAssertTrue(gate.awaitArrival(), "\(held): the start is held")
            let stopped = ReflexRuntimeHost.stop(run: runId, reason: StopReason.request)
            XCTAssertEqual(stopped["state"] as? String, "stopped", "\(held): the stop found the starting run")
            XCTAssertEqual(stopped["runId"] as? String, runId, held)
            let postedAtStop = poster.events.count
            gate.open()
            XCTAssertTrue(eventually { outcome.value != nil }, held)
            XCTAssertEqual(outcome.value, "stopped", "\(held): the start ended without publishing")
            XCTAssertEqual(ReflexRuntimeHost.status(run: runId)["state"] as? String, "stopped", "\(held): the run stands ended")
            XCTAssertNil(hand.snapshot.holder, "\(held): the hand was given back")
            XCTAssertEqual(poster.events.count, postedAtStop, "\(held): nothing posted after the stop")
            XCTAssertEqual(poster.presses.count, 0, held)
            XCTAssertEqual(ReflexRuntimeHost.stop(run: runId, reason: StopReason.request)["state"] as? String, "stopped",
                           "\(held): the same stop again answers the same end")
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
            },
            alarm: { ManualAlarm() }
        )
        let policy = String(decoding: try ReflexFixtures.policyWire(), as: UTF8.self)
        let claimed = String(decoding: try ReflexFixtures.capabilityWire(liveReflex: true), as: UTF8.self)
        var runs = 0
        func start(target: String, surface: String = "macos_desktop") -> String? {
            runs += 1
            do {
                let wire = try ReflexFixtures.wire { $0["scope"] = ["surface": surface, "target": target] }
                _ = try Provider().handle(method: "reflexStart", params: [
                    "runId": .string("binding\(runs)"), "plan": .string(String(decoding: wire, as: UTF8.self)), "limits": .string(limits),
                    "perception": .string(perception), "runPolicy": .string(policy), "capability": .string(claimed),
                    "eye": eye, "display": .number(0),
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
        XCTAssertEqual(ReflexRuntimeHost.status(run: "binding\(runs)")["state"] as? String, "stopped", "the start that failed stands ended")
        XCTAssertEqual(ReflexRuntimeHost.status(run: "never-started")["state"] as? String, ReflexRuntimeHost.missing)
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

// MARK: - The run's policy, its kernel and its deadline (t-9205 K)

/// The window's session count as its guard table hands it to the helper
/// (`COMPUTER_BUDGET_ACTIONS_PER_SESSION`, `guard_table()`).
let windowSessionCount = 5_000

/// The operator's ledger as a run's two roads read it (`OperatorGuardHost.admission`
/// and `.standing`): the real `OperatorLedger` under an unlimited pace and a session
/// count — one action counted per admission, a standing read from a copy that counts
/// nothing.
final class TestLedger: @unchecked Sendable {
    private let lock = NSLock()
    private var ledger: OperatorLedger
    private var reads = 0
    /// Runs inside a standing read, after the copy is taken and before its answer.
    var duringStanding: (() -> Void)?

    init(perSession: Int = windowSessionCount) {
        ledger = OperatorLedger(budget: ActionBudget(pace: .unlimited, perSession: perSession))
    }

    /// Count `count` actions, as earlier requests of the session did.
    func spend(_ count: Int) {
        lock.lock()
        for _ in 0..<count { _ = ledger.admit(now: 0) }
        lock.unlock()
    }

    func admit() -> GuardAdmission {
        lock.lock()
        defer { lock.unlock() }
        return ledger.admit(now: 0)
    }

    func standing() -> GuardAdmission {
        lock.lock()
        var copy = ledger
        reads += 1
        let step = duringStanding
        lock.unlock()
        step?()
        return copy.admit(now: 0)
    }

    var actions: Int {
        lock.lock()
        defer { lock.unlock() }
        return ledger.actions
    }

    var standingReads: Int {
        lock.lock()
        defer { lock.unlock() }
        return reads
    }
}

/// An alarm only the test rings — on a thread of its own, as the platform's queue does.
final class ManualAlarm: ReflexAlarm, @unchecked Sendable {
    final class Bell: ReflexAlarmBell, @unchecked Sendable {
        private let lock = NSLock()
        private var cancelled = false
        func cancel() {
            lock.lock()
            cancelled = true
            lock.unlock()
        }
        var isCancelled: Bool {
            lock.lock()
            defer { lock.unlock() }
            return cancelled
        }
    }

    private let lock = NSLock()
    private var bells: [(atNs: UInt64, ring: @Sendable () -> Void, bell: Bell)] = []

    func set(atNs: UInt64, ring: @escaping @Sendable () -> Void) -> any ReflexAlarmBell {
        let bell = Bell()
        lock.lock()
        bells.append((atNs, ring, bell))
        lock.unlock()
        return bell
    }

    /// Every deadline set so far, in order.
    var deadlines: [UInt64] {
        lock.lock()
        defer { lock.unlock() }
        return bells.map(\.atNs)
    }

    /// Ring every bell due by `now` that was not cancelled, each on a thread of its
    /// own; answers once they have rung.
    @discardableResult
    func ring(through now: UInt64) -> Int {
        lock.lock()
        let due = bells.filter { $0.atNs <= now && !$0.bell.isCancelled }
        lock.unlock()
        let rung = DispatchGroup()
        for bell in due {
            rung.enter()
            Thread.detachNewThread {
                bell.ring()
                rung.leave()
            }
        }
        _ = rung.wait(timeout: .now() + 10)
        return due.count
    }
}

/// A start through the helper's host with the window's tables, a run policy, the
/// capability table and the test's hand and ledger.
struct HostStart {
    static let eye = EyeConfig(framesPerSecond: 30, changesKept: 512, idleStopMs: 30_000, firstFrameMs: 2_000,
                               ocrCellPoints: 24, ocrMarginPoints: 4, ocrMaxShare: 0.5, ocrMaxPieces: 4)!
    var runId: String
    var plan: Data
    var hand: OperatorHand
    var policy: Data?
    var capability: Data?
    var ledger = TestLedger()

    init(runId: String, plan: Data, hand: OperatorHand, policy: Data? = nil, capability: Data? = nil) {
        self.runId = runId
        self.plan = plan
        self.hand = hand
        self.policy = policy
        self.capability = capability
    }

    func start() throws -> [String: Any] {
        let ledger = self.ledger
        return try ReflexRuntimeHost.start(
            runId: runId, plan: plan, limits: ReflexFixtures.limitsWire(), perception: ReflexFixtures.perceptionWire(),
            runPolicy: try policy ?? ReflexFixtures.policyWire(), capability: try capability ?? ReflexFixtures.capabilityWire(liveReflex: true),
            eye: Self.eye, display: 0, hand: hand, admit: { ledger.admit() }, standing: { ledger.standing() },
            actingScope: { ReflexActingScope(surface: $0.surface, target: $0.target, pid: 4_242) }
        )
    }
}

/// A hand in virtual time whose monitor hears it at once, for runs through the host.
struct HostHand {
    let clock = FakeClock(DispatchTime.now().uptimeNanoseconds)
    let poster = RecordingPoster()
    let sleeper: ScriptedSleeper
    let hand: OperatorHand
    let monitor = SyncEchoMonitor()

    init() {
        sleeper = ScriptedSleeper(clock: clock)
        hand = OperatorHand(poster: poster, clock: clock, sleeper: sleeper, tag: 0x4057)
        let hand = self.hand
        let monitor = self.monitor
        let me = Int64(getpid())
        poster.onPost = { _, tag in monitor.hear(InputOrigin.of(userData: tag, sourcePid: me, handTag: hand.tag, handPid: me)) }
    }
}

/// A frame source that draws each capture: black ground and, when the scene says so,
/// the red ball in valid_basic's ROI — pixels the real kernel reads. It keeps which
/// capture each of the run's reads took, as `SteppedFrames` does.
final class DrawnFrames: ReflexFrameSource, ReflexRunSource, @unchecked Sendable {
    static let width = 800
    static let height = 500

    private final class Buffer {
        let base: UnsafeMutableRawPointer
        init(ball: ReflexRoi?) {
            let bytes = DrawnFrames.width * DrawnFrames.height * 4
            base = UnsafeMutableRawPointer.allocate(byteCount: bytes, alignment: 16)
            base.initializeMemory(as: UInt8.self, repeating: 0, count: bytes)
            guard let ball else { return }
            for y in Int(ball.y)..<Int(ball.y + ball.height) {
                for x in Int(ball.x)..<Int(ball.x + ball.width) {
                    // BGRA: pure red, opaque.
                    base.storeBytes(of: 0xFF, toByteOffset: (y * DrawnFrames.width + x) * 4 + 2, as: UInt8.self)
                    base.storeBytes(of: 0xFF, toByteOffset: (y * DrawnFrames.width + x) * 4 + 3, as: UInt8.self)
                }
            }
        }
        deinit { base.deallocate() }
    }

    private let lock = NSLock()
    private var newestCapture: (capture: ReflexCapture, buffer: Buffer)?
    private var wake: (@Sendable () -> Void)?
    private var reads: [UInt64] = []

    func publish(_ capture: ReflexCapture, ball: ReflexRoi?) {
        let buffer = Buffer(ball: ball)
        lock.lock()
        newestCapture = (capture, buffer)
        let wake = self.wake
        lock.unlock()
        wake?()
    }

    func poke() {
        lock.lock()
        let wake = self.wake
        lock.unlock()
        wake?()
    }

    func taken(_ seq: UInt64) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return reads.filter { $0 >= seq }.count
    }

    func newest() -> ReflexFrameLoan? {
        lock.lock()
        let held = newestCapture
        if let held { reads.append(held.capture.captureSeq) }
        lock.unlock()
        guard let (capture, buffer) = held else { return nil }
        return ReflexFrameLoan(capture: capture) { body in
            body(ReflexPixels(base: UnsafeRawPointer(buffer.base), width: Self.width, height: Self.height, bytesPerRow: Self.width * 4))
            return withExtendedLifetime(buffer) { true }
        }
    }

    func onCapture(_ wake: (@Sendable () -> Void)?) {
        lock.lock()
        self.wake = wake
        lock.unlock()
    }

    func end() {}
}

/// The real kernel, its tick running out while it reads the captures marked late:
/// the admission's clock read says now, every later read is past the deadline —
/// astra's t-6768 repro, inside a run.
final class LateKernel: ReflexPerceptionKernel, @unchecked Sendable {
    private let lock = NSLock()
    private var late: Set<UInt64> = []
    let inner = PerceptionKernel()

    func late(on capture: UInt64) {
        lock.lock()
        late.insert(capture)
        lock.unlock()
    }

    fileprivate func isLate(_ capture: UInt64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return late.contains(capture)
    }

    final class Session: ReflexPerceptionSession {
        let inner: any ReflexPerceptionSession
        let kernel: LateKernel

        init(inner: any ReflexPerceptionSession, kernel: LateKernel) {
            self.inner = inner
            self.kernel = kernel
        }

        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                     budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
            guard kernel.isLate(frame.capture_seq) else { return inner.observe(frame: frame, pixels: pixels, hand: hand, budget: &budget) }
            let deadline = budget.deadlineHostNs
            let reads = Box<Int>()
            var slow = ReflexPerceptionBudget(samples: budget.samples, deadlineHostNs: deadline) {
                let read = reads.value ?? 0
                reads.value = read + 1
                return read == 0 ? deadline - 1 : deadline + 6_000_000
            }
            return inner.observe(frame: frame, pixels: pixels, hand: hand, budget: &slow)
        }
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        Session(inner: try inner.session(for: plan, limits: limits), kernel: self)
    }
}

/// A kernel that keeps what each session was opened with, the thread class it was
/// opened and read on, every session it made and the hand's point each capture was
/// read near; it answers the scene.
final class RecordingKernel: ReflexPerceptionKernel, @unchecked Sendable {
    let scene: SceneKernel
    private let lock = NSLock()
    private var opened: [(limits: PerceptionLimits, session: Int, qos: qos_class_t)] = []
    private var readQos: [qos_class_t] = []
    /// Each session's number and the runs whose frames it read.
    private var runsRead: [Int: Set<String>] = [:]
    /// The hand's point each capture was read near, as `[x, y]`; nil for none.
    private var readNear: [UInt64: [Int64]?] = [:]

    init(scene: SceneKernel = SceneKernel()) { self.scene = scene }

    final class Session: ReflexPerceptionSession {
        let inner: any ReflexPerceptionSession
        let kernel: RecordingKernel
        let number: Int
        init(inner: any ReflexPerceptionSession, kernel: RecordingKernel, number: Int) {
            self.inner = inner
            self.kernel = kernel
            self.number = number
        }

        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                     budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
            kernel.noteRead(qos_class_self(), session: number, run: frame.run_id, capture: frame.capture_seq, near: hand)
            return inner.observe(frame: frame, pixels: pixels, hand: hand, budget: &budget)
        }
    }

    fileprivate func noteRead(_ qos: qos_class_t, session: Int, run: String, capture: UInt64, near hand: (x: Int64, y: Int64)?) {
        lock.lock()
        readQos.append(qos)
        runsRead[session, default: []].insert(run)
        readNear[capture] = .some(hand.map { [$0.x, $0.y] })
        lock.unlock()
    }

    /// The hand's point each capture was read near, by capture: `[x, y]`, or nil for none.
    var near: [UInt64: [Int64]?] {
        lock.lock()
        defer { lock.unlock() }
        return readNear
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        lock.lock()
        let number = opened.count + 1
        opened.append((limits, number, qos_class_self()))
        lock.unlock()
        return Session(inner: try scene.session(for: plan, limits: limits), kernel: self, number: number)
    }

    var sessions: [(limits: PerceptionLimits, session: Int, qos: qos_class_t)] {
        lock.lock()
        defer { lock.unlock() }
        return opened
    }

    /// The runs each session read frames of, by the session's number.
    var runsBySession: [Int: Set<String>] {
        lock.lock()
        defer { lock.unlock() }
        return runsRead
    }

    var reads: [qos_class_t] {
        lock.lock()
        defer { lock.unlock() }
        return readQos
    }
}

extension ReflexFixtures {
    /// valid_basic's click on the ball, its rule firing on the ball being there under a
    /// colour spec that confirms after `confirm` captures in agreement.
    static func clickPlan(maxFires: Int = 1, confirm: Int) throws -> ValidatedReflexPlan {
        try plan { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [["id": "click1", "kind": "click", "target": "ball"]]]]
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["max_fires"] = maxFires
            rules[0]["cooldown_ms"] = 0
            rules[0]["predicate"] = ["op": "eq", "value": 1]
            object["rules"] = rules
            var detectors = object["detectors"] as! [[String: Any]]
            var color = detectors[0]["color"] as! [String: Any]
            color["confirm"] = confirm
            detectors[0]["color"] = color
            object["detectors"] = detectors
        }
    }
}

final class ReflexKernelAndPolicyTests: XCTestCase {
    // MARK: K — the kernel, installed and read as the window sent it

    /// The helper's launch installs the perception kernel in one place: before it,
    /// a start is refused for want of one and the handshake says so; after it, the
    /// handshake names a kernel and the run policy the helper reads.
    func test_the_helper_installs_the_kernel_at_launch() throws {
        let saved = ReflexRuntimeHost.kernel
        defer { ReflexRuntimeHost.kernel = saved }
        ReflexRuntimeHost.kernel = nil
        XCTAssertEqual(ReflexRuntimeHost.handshake()["kernel"] as? Bool, false)
        var guarded = 0
        HelperLaunch.install(operatorGuard: { guarded += 1 })
        XCTAssertEqual(guarded, 1, "the operator's guard is stood up by the same launch")
        XCTAssertTrue(ReflexRuntimeHost.kernel is PerceptionKernel, "the kernel the helper's runs read with is R5's")
        let handshake = ReflexRuntimeHost.handshake()
        XCTAssertEqual(handshake["kernel"] as? Bool, true)
        XCTAssertEqual(handshake["planVersion"] as? Int, Int(ReflexContract.version))
        XCTAssertEqual(handshake["runPolicy"] as? Int, Int(ReflexContract.runPolicyVersion))
    }

    /// The perception table the window sends (`game_state::limits_wire`, the shared
    /// golden) reaches the kernel's session field for field — the tick's samples and
    /// time, the anchors — with no number of the helper's own in between.
    func test_the_perception_table_reaches_the_kernel_unchanged() throws {
        let saved = (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel)
        defer { (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel) = saved }
        let host = HostHand()
        let kernel = RecordingKernel()
        ReflexRuntimeHost.kernel = kernel
        ReflexRuntimeHost.parts = .init(reader: { _, _, _, _ in SteppedFrames() }, monitor: { _ in host.monitor },
                                        boundary: { _ in PermitEverywhere() }, alarm: { ManualAlarm() })
        _ = try HostStart(runId: "tables", plan: try ReflexFixtures.wire(), hand: host.hand).start()
        defer { _ = ReflexRuntimeHost.stop(run: "tables", reason: StopReason.request) }
        let sent = try PerceptionSpecs.decodeLimits(try ReflexFixtures.perceptionWire())
        XCTAssertEqual(kernel.sessions.map(\.limits), [sent])
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: try ReflexFixtures.perceptionWire()) as? [String: UInt64])
        XCTAssertEqual(kernel.sessions.first?.limits.max_tick_samples, object["max_tick_samples"])
        XCTAssertEqual(kernel.sessions.first?.limits.max_tick_ns, object["max_tick_ns"])
        XCTAssertEqual(kernel.sessions.first?.limits.max_anchors, object["max_anchors"])
    }

    /// The run's evaluator — where the kernel's session is opened and every frame is
    /// read — and its hand run at user-interactive QoS (R5's ABBA: default QoS put 14
    /// of 800 ticks past 5 ms, user-interactive none).
    func test_the_evaluator_runs_at_user_interactive_qos() throws {
        let scene = SceneKernel()
        let kernel = RecordingKernel(scene: scene)
        let handQos = Box<qos_class_t>()
        let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(), scene: scene, kernel: kernel)
        rig.sleeper.onSleep = { _, _ in
            if handQos.value == nil { handQos.value = qos_class_self() }
            rig.frame(.ball(track: 5, box: LiveRig.box))
        }
        try rig.session.start()
        rig.frame(.ball(track: 5, box: LiveRig.box))
        _ = rig.receipts(1)
        rig.session.stop(reason: StopReason.request)
        XCTAssertEqual(kernel.sessions.map(\.qos), [QOS_CLASS_USER_INTERACTIVE], "the session is opened on the evaluator")
        XCTAssertFalse(kernel.reads.isEmpty)
        XCTAssertTrue(kernel.reads.allSatisfy { $0 == QOS_CLASS_USER_INTERACTIVE }, "every frame is read at user-interactive QoS")
        XCTAssertEqual(handQos.value, QOS_CLASS_USER_INTERACTIVE, "the hand's thread too")
    }

    /// The real kernel reads a capture its tick's deadline passed during: that capture
    /// answers `unknown(budget)` and the kernel forgets its confirmation — so the Ready
    /// sighting of the capture before it gives a leaf decided there nothing more, and one
    /// normal capture after it is not enough for a spec that confirms on two.
    func test_a_budget_unknown_never_revives_old_ready() throws {
        let frames = DrawnFrames()
        let kernel = LateKernel()
        let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(maxFires: 2, confirm: 2), source: frames, kernel: kernel)
        let ball = ReflexRoi(x: 8, y: 8, width: 12, height: 12, space: .pixel)
        var seq: UInt64 = 0
        func publish(_ shown: ReflexRoi?, late: Bool = false) {
            seq += 1
            if late { kernel.late(on: seq) }
            frames.publish(ReflexFixtures.capture(seq, capturedNs: rig.clock.nowNs()), ball: shown)
            rig.settle(seq, taken: frames.taken, poke: frames.poke)
        }
        func ballSeen() -> ReflexObservation? { rig.session.sightings.latest()?.byDetector["ball"] }
        // The leaf decided on capture 2 sleeps before its first waypoint: capture 3,
        // read late, is published then.
        rig.sleeper.onSleep = { index, _ in if index == 0 { publish(ball, late: true) } }
        try rig.session.start()
        let echoed = rig.poster.events.count
        publish(ball)
        XCTAssertEqual(ballSeen()?.unknown, .unconfirmed, "one capture of a spec that confirms on two")
        publish(ball)
        let decided = try XCTUnwrap(rig.receipts(1).first)
        XCTAssertEqual(decided.sourceCapture, 2, "decided on the Ready capture 2")
        XCTAssertEqual(decided.outcome, .moved, "capture 3 answered budget: the target decided on is not there any more")
        XCTAssertEqual(rig.poster.events.count, echoed, "nothing posted on capture 2's word once capture 3 read late")
        XCTAssertEqual(ballSeen()?.unknown, .budget)
        XCTAssertNil(ballSeen()?.value)
        XCTAssertEqual(rig.session.status.sightings.first?.unknown, .budget, "the status names the late read")
        publish(ball)
        XCTAssertEqual(ballSeen()?.unknown, .unconfirmed, "the late read's forgetting holds: one capture after it is not two")
        publish(ball)
        XCTAssertEqual(ballSeen()?.value, 1, "two in agreement again")
        XCTAssertEqual(rig.session.status.fires, 1)
        rig.session.stop(reason: StopReason.request)
    }

    /// Every run gets its own kernel session — never the one before it, whether that
    /// run was stopped or reached its deadline — and a run ended by its deadline frees
    /// the helper for the next start.
    func test_a_new_run_gets_a_new_kernel_session() throws {
        let saved = (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel)
        defer { (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel) = saved }
        let host = HostHand()
        let kernel = RecordingKernel()
        let alarm = ManualAlarm()
        let frames = SteppedFrames()
        ReflexRuntimeHost.kernel = kernel
        ReflexRuntimeHost.parts = .init(reader: { _, _, _, _ in frames }, monitor: { _ in host.monitor },
                                        boundary: { _ in PermitEverywhere() }, alarm: { alarm })
        var seq: UInt64 = 0
        /// One capture, read by whichever run stands.
        func read() {
            seq += 1
            let taken = seq
            frames.publish(ReflexFixtures.capture(taken, capturedNs: host.clock.nowNs()))
            _ = eventually { frames.taken(taken) >= 1 }
        }
        let wire = try ReflexFixtures.wire()
        _ = try HostStart(runId: "first", plan: wire, hand: host.hand).start()
        read()
        XCTAssertEqual(ReflexRuntimeHost.stop(run: "first", reason: StopReason.request)["state"] as? String, "stopped")
        let short: UInt64 = 5_000_000_000
        _ = try HostStart(runId: "second", plan: wire, hand: host.hand, policy: try ReflexFixtures.policyWire(runNs: short)).start()
        read()
        XCTAssertThrowsError(try HostStart(runId: "third", plan: wire, hand: host.hand).start(), "one run at a time") { error in
            XCTAssertEqual((error as? ProviderError)?.code, "hand_busy")
        }
        XCTAssertEqual(alarm.ring(through: host.clock.nowNs() + short), 1, "the second run's deadline rings")
        XCTAssertTrue(eventually { ReflexRuntimeHost.status(run: "second")["reason"] as? String == ReflexRunReason.deadline })
        XCTAssertNil(host.hand.snapshot.holder, "the deadline gave the hand back")
        XCTAssertNoThrow(try HostStart(runId: "third", plan: wire, hand: host.hand).start(), "the deadline freed the helper for the next start")
        defer { _ = ReflexRuntimeHost.stop(run: "third", reason: StopReason.request) }
        read()
        XCTAssertEqual(kernel.sessions.map(\.session), [1, 2, 3], "one session a run")
        XCTAssertTrue(eventually { kernel.runsBySession.count == 3 })
        XCTAssertEqual(kernel.runsBySession, [1: ["first"], 2: ["second"], 3: ["third"]], "never a session another run read with")
    }

    /// A status names each detector's newest admissible word — its value or why it is
    /// unknown, the track it points at and its frame's age — in plan order, at most the
    /// table's detectors, and never the receipts.
    func test_status_names_each_latest_sighting() throws {
        let saved = (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel)
        defer { (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel) = saved }
        let host = HostHand()
        let frames = SteppedFrames()
        let kernel = PairKernel()
        ReflexRuntimeHost.kernel = kernel
        ReflexRuntimeHost.parts = .init(reader: { _, _, _, _ in frames }, monitor: { _ in host.monitor },
                                        boundary: { _ in PermitEverywhere() }, alarm: { ManualAlarm() })
        let wire = try ReflexFixtures.wire { object in
            var detectors = object["detectors"] as! [[String: Any]]
            var other = detectors[0]
            other["id"] = "flag"
            other["roi"] = ["height": 32, "space": "pixel", "width": 32, "x": 64, "y": 0]
            detectors.append(other)
            object["detectors"] = detectors
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["predicate"] = ["op": "eq", "value": 9]
            object["rules"] = rules
        }
        _ = try HostStart(runId: "seeing", plan: wire, hand: host.hand).start()
        defer { _ = ReflexRuntimeHost.stop(run: "seeing", reason: StopReason.request) }
        frames.publish(ReflexFixtures.capture(1, capturedNs: host.clock.nowNs()))
        XCTAssertTrue(eventually { (ReflexRuntimeHost.status(run: "seeing")["sightings"] as? [[String: Any]])?.count == 2 })
        host.clock.set(host.clock.nowNs() + 3_000_000)
        let status = ReflexRuntimeHost.status(run: "seeing")
        let sightings = try XCTUnwrap(status["sightings"] as? [[String: Any]])
        XCTAssertEqual(sightings.map { $0["detector"] as? String }, ["ball", "flag"], "plan order")
        XCTAssertEqual(sightings[0]["value"] as? Int64, 1)
        XCTAssertTrue(sightings[0]["unknown"] is NSNull)
        XCTAssertEqual(sightings[0]["track"] as? UInt64, 5)
        XCTAssertEqual(sightings[0]["ageNs"] as? UInt64, 3_000_000, "how old the frame is now")
        XCTAssertTrue(sightings[1]["value"] is NSNull)
        XCTAssertEqual(sightings[1]["unknown"] as? String, "occluded")
        XCTAssertTrue(sightings[1]["track"] is NSNull)
        XCTAssertLessThanOrEqual(UInt64(sightings.count), try ReflexFixtures.limits().max_detectors)
        XCTAssertNil(status["receipts"], "a status carries no receipts")
    }

    /// A kernel that picks by nearness picks near where the run's last fire sent the hand
    /// (t-10223 R8): every read is handed the point of the target that fire's last leaf acts
    /// on, as the frame it fired on showed it — nothing before the first fire, and the newer
    /// fire's point from the capture after it.
    func test_the_kernel_reads_near_where_the_last_fire_sent_the_hand() throws {
        let kernel = RecordingKernel()
        let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(maxFires: 2, pick: .nearest), scene: kernel.scene, kernel: kernel)
        let shown = Box<ReflexRoi>()
        rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: rig.track, box: shown.value ?? LiveRig.box)) }
        try rig.session.start()
        defer { rig.session.stop(reason: StopReason.request) }
        let first = rig.frame(.ball(track: rig.track, box: LiveRig.box))
        XCTAssertEqual(rig.receipts(1).first?.outcome, .done)
        // The ball goes, and comes back elsewhere on another track: a new edge, a second fire.
        let elsewhere = ReflexRoi(x: 16, y: 16, width: 16, height: 16, space: .pixel)
        shown.value = elsewhere
        rig.track = 6
        rig.poster.movePointer(to: SmoothPointerPath.Point(x: 0, y: 0))
        rig.frame(.absent)
        let second = rig.frame(.ball(track: rig.track, box: elsewhere))
        XCTAssertEqual(rig.receipts(1).first?.outcome, .done)
        let after = rig.frame(.ball(track: rig.track, box: elsewhere))
        let near = kernel.near
        XCTAssertEqual(near[first], .some(nil), "no fire before the first capture's read")
        for capture in (first + 1)...second {
            XCTAssertEqual(near[capture], .some([16, 16]), "capture \(capture): the first fire's target, the ball's middle")
        }
        for capture in (second + 1)...after {
            XCTAssertEqual(near[capture], .some([24, 24]), "capture \(capture): the second fire's target")
        }
        XCTAssertEqual(rig.session.status.fires, 2)
    }

    /// A receipt names how its leaf's detector picks a target that follows none, and the
    /// track its leaf was decided on (t-10223 R8) — `first` for a plan that names no pick.
    func test_a_receipt_names_its_pick_and_its_track() throws {
        for pick in [ReflexPick.first, .nearest] {
            let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(pick: pick))
            rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: rig.track, box: LiveRig.box)) }
            try rig.session.start()
            rig.frame(.ball(track: rig.track, box: LiveRig.box))
            let receipt = try XCTUnwrap(rig.receipts(1).first, pick.rawValue)
            XCTAssertEqual(receipt.outcome, .done, pick.rawValue)
            XCTAssertEqual(receipt.pick, pick)
            XCTAssertEqual(receipt.trackId, rig.track, pick.rawValue)
            XCTAssertEqual(receipt.targetId, "ball#\(rig.track)", pick.rawValue)
            rig.session.stop(reason: StopReason.request)
        }
    }

    /// The window's session closing takes the stop's own road before the helper exits:
    /// a reflex click held in its fence is let go of, and the run ends, before exit.
    func test_a_closed_session_lets_go_before_exit() throws {
        let saved = (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel)
        defer {
            (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel) = saved
            OperatorGuardHost.resume(resetBudget: false)
        }
        let host = HostHand()
        let frames = SteppedFrames()
        let kernel = SceneKernel()
        ReflexRuntimeHost.kernel = kernel
        ReflexRuntimeHost.parts = .init(reader: { _, _, _, _ in frames }, monitor: { _ in host.monitor },
                                        boundary: { _ in PermitEverywhere() }, alarm: { ManualAlarm() })
        let order = Box<[String]>()
        order.value = []
        let inFence = Gate()
        inFence.close()
        var seq: UInt64 = 0
        func publish() {
            seq += 1
            kernel.show(.ball(track: 5, box: LiveRig.box), on: seq)
            frames.publish(ReflexFixtures.capture(seq, capturedNs: host.clock.nowNs()))
            let taken = seq
            _ = eventually { frames.taken(taken) >= 1 }
            frames.poke()
            _ = eventually { frames.taken(taken) >= 2 }
        }
        host.sleeper.onSleep = { index, _ in
            if index < 10 { publish() } else if index == 10 { inFence.pass() }
        }
        host.poster.onPost = { [monitor = host.monitor, hand = host.hand] event, tag in
            let me = Int64(getpid())
            monitor.hear(InputOrigin.of(userData: tag, sourcePid: me, handTag: hand.tag, handPid: me))
            if case .buttonUp = event.kind { order.value?.append("release") }
        }
        let wire = try ReflexFixtures.wire { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [["id": "click1", "kind": "click", "target": "ball"]]]]
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["cooldown_ms"] = 0
            rules[0]["predicate"] = ["op": "eq", "value": 1]
            object["rules"] = rules
        }
        _ = try HostStart(runId: "closing", plan: wire, hand: host.hand).start()
        publish()
        XCTAssertTrue(inFence.awaitArrival(), "the click is in its fence")
        XCTAssertEqual(host.hand.snapshot.held, [.button(.left)])
        OperatorGuardHost.sessionClosed { order.value?.append("exit") }
        XCTAssertEqual(order.value, ["release", "exit"], "let go of before the helper exits")
        XCTAssertEqual(ReflexRuntimeHost.status(run: "closing")["state"] as? String, "stopped")
        XCTAssertEqual(ReflexRuntimeHost.status(run: "closing")["reason"] as? String, StopReason.sessionClosed)
        inFence.open()
        XCTAssertNil(host.hand.snapshot.holder)
    }

    // MARK: K — a spent quota comes back, and only it

    /// valid_basic's ball under two rules: `follow` fires on it, `never` on a value the
    /// ball never has.
    private func twoRulePlan(maxFires: Int = 1) throws -> ValidatedReflexPlan {
        try ReflexFixtures.plan { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [["id": "click1", "kind": "click", "target": "ball"]]]]
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["max_fires"] = maxFires
            rules[0]["cooldown_ms"] = 8
            rules[0]["predicate"] = ["op": "eq", "value": 1]
            var never = rules[0]
            never["id"] = "never"
            never["predicate"] = ["op": "eq", "value": 7]
            never["priority"] = 0
            rules.append(never)
            object["rules"] = rules
        }
    }

    /// Renewal gives a spent rule its fire count back and nothing else: an edge read
    /// while spent is used up, a true that lasts, the same capture, an unknown turning
    /// true and a cooldown still running fire nothing after it, a rule that never fires
    /// holds nobody back, and a refused renewal renews nothing. On a run, the quota comes
    /// back twice on new edges while the admissions, the kernel's tracks and the evidence
    /// count carry on as they were.
    func test_a_renewal_restores_only_the_spent_quota() throws {
        let plan = try twoRulePlan()
        var book = ReflexRuleBook(plan, policy: try ReflexFixtures.policy(renew: true))
        var asked = 0
        var allow = true
        func read(_ capture: UInt64, _ value: Int64?, ms: UInt64, free: Bool = true) -> (fired: String?, renewed: [String]) {
            let read = book.read(streamEpoch: 1, captureSeq: capture, values: value.map { ["ball": $0] } ?? [:], nowNs: ms * 1_000_000,
                                 handFree: free, renewal: { asked += 1; return allow })
            return (read.fired?.id, read.renewed.map(\.rule))
        }
        XCTAssertEqual(read(1, 1, ms: 0).fired, "follow", "armed at the start")
        XCTAssertNil(read(2, 1, ms: 1, free: false).fired)
        XCTAssertEqual(read(3, 0, ms: 2, free: false).renewed, [], "the hand is busy: nothing comes back")
        XCTAssertNil(read(4, 1, ms: 3, free: false).fired, "an edge read while spent")
        XCTAssertEqual(asked, 0, "no renewal asked while the hand is busy")
        let back = read(5, 1, ms: 4)
        XCTAssertEqual(back.renewed, ["follow"], "the spent quota comes back once the hand is free")
        XCTAssertNil(back.fired, "…but the edge read while spent was used up")
        XCTAssertEqual(asked, 1)
        XCTAssertEqual(read(5, 1, ms: 5).renewed, [], "the same capture is not read again")
        XCTAssertNil(read(6, 1, ms: 6).fired, "a true that lasts is no edge")
        XCTAssertNil(read(7, nil, ms: 7).fired)
        XCTAssertNil(read(8, 1, ms: 8).fired, "unknown turning true is no edge")
        XCTAssertNil(read(9, 0, ms: 9).fired)
        XCTAssertNil(read(10, 1, ms: 7).fired, "armed, but the cooldown from the first fire still runs")
        XCTAssertNil(read(11, 0, ms: 12).fired)
        XCTAssertEqual(read(12, 1, ms: 20).fired, "follow", "a new edge after the cooldown")
        XCTAssertEqual(read(13, 0, ms: 30).renewed, ["follow"], "never held back by a rule that never fires")
        XCTAssertEqual(read(14, 1, ms: 40).fired, "follow", "a false read after the renewal, then true: an edge")
        allow = false
        XCTAssertNil(read(15, 0, ms: 50).fired)
        XCTAssertEqual(read(16, 1, ms: 60).renewed, [], "a refused renewal gives nothing back")
        XCTAssertNil(read(17, 0, ms: 61).fired)
        XCTAssertNil(read(18, 1, ms: 70).fired, "and fires nothing")
        XCTAssertEqual(book.fires, 3)
        XCTAssertEqual(book.renewals, 2)

        // On a run: three fires on three edges, the quota back twice.
        let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(maxFires: 1), policy: try ReflexFixtures.policy(renew: true))
        rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: rig.track, box: LiveRig.box)) }
        try rig.session.start()
        var evidence: [UInt64] = []
        for fire in 1...3 {
            rig.track = UInt64(4 + fire)
            rig.awayAndBack()
            XCTAssertEqual(rig.receipts(1).first?.outcome, .done, "fire \(fire)")
            evidence.append(rig.session.sightings.latest()?.evidence ?? .max)
        }
        let status = rig.session.status
        XCTAssertEqual(status.fires, 3)
        XCTAssertEqual(status.renewals, 2, "back twice")
        XCTAssertEqual(status.leaves, 3)
        XCTAssertEqual(rig.ledger.actions, 3, "each leaf admitted once, and a renewal admits nothing")
        XCTAssertGreaterThanOrEqual(rig.ledger.standingReads, 2, "a renewal reads the ledger's standing")
        XCTAssertEqual(Set(evidence), [0], "no evidence was taken back by a renewal")
        XCTAssertEqual(rig.downs, 3)
        XCTAssertEqual(rig.ups, 3)
        XCTAssertEqual(rig.hand.snapshot.unconfirmedReleases, 0)
        XCTAssertEqual(rig.alarm.deadlines, [rig.deadlineNs], "one deadline, never lengthened")
        rig.session.stop(reason: StopReason.request)
    }

    /// A renewal reads the operator's ledger and counts nothing: at 4,999 of the
    /// window's 5,000 the quota comes back and its one leaf is the 5,000th action; at
    /// 5,000 the next renewal is refused, the run ends saying so, and the count stands.
    func test_a_renewal_is_not_an_admission_and_ends_at_the_session_count() throws {
        let ledger = TestLedger()
        ledger.spend(windowSessionCount - 2)
        let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(maxFires: 1), policy: try ReflexFixtures.policy(renew: true), ledger: ledger)
        rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: rig.track, box: LiveRig.box)) }
        try rig.session.start()
        rig.frame(.ball(track: rig.track, box: LiveRig.box))
        XCTAssertEqual(rig.receipts(1).first?.outcome, .done)
        XCTAssertEqual(ledger.actions, windowSessionCount - 1)
        rig.track = 6
        rig.awayAndBack()
        XCTAssertEqual(rig.receipts(1).first?.outcome, .done, "back at 4,999: its leaf is the 5,000th action")
        XCTAssertEqual(ledger.actions, windowSessionCount)
        rig.track = 7
        rig.awayAndBack(waiting: false)
        XCTAssertTrue(eventually { rig.session.status.state == .stopped(StopReason.sessionBudget) }, "\(rig.session.status.state)")
        XCTAssertEqual(ledger.actions, windowSessionCount, "the session count stands: a renewal neither counts nor resets it")
        XCTAssertEqual(rig.session.status.renewals, 1)
        XCTAssertEqual(rig.downs, 2, "nothing pressed after the refusal")
        XCTAssertNil(rig.hand.snapshot.holder)
    }

    /// A renewal that meets a stop while the ledger is read, a monitor that stopped
    /// hearing or a display that changed under the run grants nothing, and no leaf
    /// follows.
    func test_a_renewal_that_meets_a_stop_a_deaf_monitor_or_a_changed_display_grants_nothing() throws {
        for (name, meet) in [("a stop", 0), ("a deaf monitor", 1), ("a changed display", 2)] {
            let ledger = TestLedger()
            let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(maxFires: 1), policy: try ReflexFixtures.policy(renew: true), ledger: ledger)
            rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: rig.track, box: LiveRig.box)) }
            try rig.session.start()
            rig.frame(.ball(track: rig.track, box: LiveRig.box))
            XCTAssertEqual(rig.receipts(1).first?.outcome, .done, name)
            var status = ReflexFrameStatus.ready
            switch meet {
            case 0: ledger.duringStanding = { rig.session.stop(reason: StopReason.request) }
            case 1: rig.monitor.interrupt("the system turned the tap off")
            default: status = .interrupted
            }
            rig.track = 6
            rig.awayAndBack(status: status, waiting: meet != 0)
            XCTAssertFalse(eventually(within: 0.3) { rig.session.status.leaves > 1 }, "\(name): no leaf")
            XCTAssertEqual(rig.session.status.renewals, 0, name)
            XCTAssertEqual(rig.downs, 1, name)
            XCTAssertEqual(ledger.actions, 1, name)
            rig.session.stop(reason: StopReason.request)
        }
    }

    /// Without renewal a rule's `max_fires` is its total for the run: a new edge after
    /// it is spent fires nothing, no renewal is ever asked, and the run stands.
    func test_legacy_policy_keeps_max_fires_total() throws {
        let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(maxFires: 1), policy: try ReflexFixtures.policy(renew: false))
        rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: rig.track, box: LiveRig.box)) }
        try rig.session.start()
        rig.frame(.ball(track: rig.track, box: LiveRig.box))
        XCTAssertEqual(rig.receipts(1).first?.outcome, .done)
        for track in 6...8 {
            rig.track = UInt64(track)
            rig.awayAndBack()
        }
        XCTAssertEqual(rig.session.status.fires, 1, "max_fires is the run's total")
        XCTAssertEqual(rig.session.status.renewals, 0)
        XCTAssertEqual(rig.ledger.standingReads, 0, "no renewal is asked")
        XCTAssertEqual(rig.downs, 1)
        XCTAssertEqual(rig.session.status.state, .running, "the run stands, spent")
        rig.session.stop(reason: StopReason.request)
    }

    /// The deadline's alarm lets go of what the run holds on a thread of its own while
    /// the hand sleeps in a click's fence, the frames have stopped and the evaluator is
    /// stuck inside the kernel: the button comes up before any of them moves, the run
    /// ends saying why, and nothing is admitted or pressed at or after the deadline.
    func test_deadline_releases_with_everything_blocked() throws {
        let scene = SceneKernel()
        let kernel = StuckKernel(scene: scene)
        let rig = try LiveRig(plan: try ReflexFixtures.clickPlan(), scene: scene, kernel: kernel)
        let inFence = Gate()
        inFence.close()
        rig.sleeper.onSleep = { index, _ in
            if index < 10 {
                rig.frame(.ball(track: 5, box: LiveRig.box))
            } else if index == 10 {
                // The frames stop and the next read sticks inside the kernel.
                kernel.stick()
                rig.frames.publish(ReflexFixtures.capture(99, capturedNs: rig.clock.nowNs()))
                inFence.pass()
            }
        }
        try rig.session.start()
        rig.frame(.ball(track: 5, box: LiveRig.box))
        XCTAssertTrue(inFence.awaitArrival(), "the click is in its fence")
        XCTAssertTrue(kernel.awaitStuck(), "the evaluator is stuck inside the kernel")
        XCTAssertEqual(rig.hand.snapshot.held, [.button(.left)])
        XCTAssertEqual(rig.alarm.ring(through: rig.deadlineNs), 1)
        XCTAssertEqual(rig.ups, 1, "let go of by the alarm while the hand, the frames and the kernel stay stuck")
        XCTAssertEqual(rig.session.status.state, .stopped(ReflexRunReason.deadline))
        kernel.unstick()
        inFence.open()
        _ = rig.receipts(1)
        XCTAssertEqual(rig.downs, 1, "nothing pressed after the deadline")
        XCTAssertNil(rig.hand.snapshot.holder)

        // At the deadline itself nothing is admitted and nothing goes.
        let leaf = try LeafRig()
        leaf.decide()
        let runner = ReflexLeafRunner(
            runId: "run", hand: leaf.hand, token: leaf.token, limits: leaf.limits,
            style: try PointerStyle(try ReflexFixtures.plan().plan.pointer, limits: leaf.limits),
            sightings: leaf.sightings, admit: { leaf.admissions.admit() },
            fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
            boundary: PermitEverywhere(), deadlineNs: leaf.clock.nowNs()
        )
        let atDeadline = runner.run(LeafRig.click, index: 0)
        XCTAssertEqual(atDeadline.outcome, .lease)
        XCTAssertEqual(leaf.admissions.admitted, 0, "not admitted at the deadline")
        XCTAssertEqual(leaf.poster.events.count, 0)
    }
}

/// Two detectors' words: the ball known on track 5, the flag occluded.
final class PairKernel: ReflexPerceptionKernel, @unchecked Sendable {
    final class Session: ReflexPerceptionSession {
        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                     budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
            guard let ref = ReflexFrameRef(frame) else { return [] }
            return [
                ReflexFixtures.ball(on: frame, track: 5, box: LiveRig.box),
                ReflexObservation(detector_id: "flag", frame: ref, unknown: .occluded, value: nil, target: nil,
                                  scale: ReflexScale(numerator: 1, denominator: 1), cells: nil, samples: 64),
            ]
        }
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession { Session() }
}

/// The scene's kernel, which the test can make stick inside its next read.
final class StuckKernel: ReflexPerceptionKernel, @unchecked Sendable {
    let scene: SceneKernel

    init(scene: SceneKernel) { self.scene = scene }

    private let gate = Gate()
    private let lock = NSLock()
    private var sticking = false

    func stick() {
        lock.lock()
        sticking = true
        lock.unlock()
        gate.close()
    }

    func awaitStuck() -> Bool { gate.awaitArrival() }

    func unstick() { gate.open() }

    fileprivate var stuck: Bool {
        lock.lock()
        defer { lock.unlock() }
        return sticking
    }

    final class Session: ReflexPerceptionSession {
        let inner: any ReflexPerceptionSession
        let kernel: StuckKernel
        init(inner: any ReflexPerceptionSession, kernel: StuckKernel) {
            self.inner = inner
            self.kernel = kernel
        }

        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                     budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
            if kernel.stuck { kernel.gate.pass() }
            return inner.observe(frame: frame, pixels: pixels, hand: hand, budget: &budget)
        }
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        Session(inner: try scene.session(for: plan, limits: limits), kernel: self)
    }
}

/// A run through the helper's host on the test's hand, frames and scene, its
/// deadline on an alarm only the test rings.
final class HostRig: @unchecked Sendable {
    let host = HostHand()
    let frames = SteppedFrames()
    let kernel = SceneKernel()
    let alarm = ManualAlarm()
    private let saved = (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel)
    private var seq: UInt64 = 0

    init() {
        ReflexRuntimeHost.kernel = kernel
        let (frames, monitor, alarm) = (self.frames, host.monitor, self.alarm)
        ReflexRuntimeHost.parts = .init(reader: { _, _, _, _ in frames }, monitor: { _ in monitor },
                                        boundary: { _ in PermitEverywhere() }, alarm: { alarm })
        host.sleeper.onSleep = { [weak self] _, _ in self?.show(.ball(track: 5, box: LiveRig.box)) }
    }

    func restore() {
        (ReflexRuntimeHost.parts, ReflexRuntimeHost.kernel) = saved
    }

    /// A click plan's wire: the ball clicked on each edge, `maxFires` a run.
    static func clickWire(maxFires: Int = 8) throws -> Data {
        try ReflexFixtures.wire { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [["id": "click1", "kind": "click", "target": "ball"]]]]
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["max_fires"] = maxFires
            rules[0]["cooldown_ms"] = 0
            rules[0]["predicate"] = ["op": "eq", "value": 1]
            object["rules"] = rules
        }
    }

    private let lock = NSLock()

    /// Publish the next capture showing `sight`, and return once the run's
    /// evaluator took it and came back for another.
    func show(_ sight: SceneKernel.Sight) {
        lock.lock()
        seq += 1
        let seq = self.seq
        lock.unlock()
        kernel.show(sight, on: seq)
        frames.publish(ReflexFixtures.capture(seq, capturedNs: host.clock.nowNs()))
        _ = eventually { frames.taken(seq) >= 1 }
        frames.poke()
        _ = eventually { frames.taken(seq) >= 2 }
    }

    var downs: Int { host.poster.presses.filter { if case .buttonDown = $0.kind { return true } else { return false } }.count }
}

final class ReflexRunRoadTests: XCTestCase {
    // MARK: 경계2 — a request names its run, and the helper compares it under one lock

    /// The window confirmed run A, then A ended and B started: a late stop and a late
    /// status for A answer A's own end and never touch B; an unknown run is `missing`;
    /// the helper's road carries the run a stop names, and a stop that names none is
    /// refused. The operator's stop still ends whatever stands.
    func test_a_late_stop_for_run_a_never_stops_run_b() throws {
        let rig = HostRig()
        defer {
            rig.restore()
            OperatorGuardHost.resume(resetBudget: false)
        }
        let wire = try HostRig.clickWire()
        _ = try HostStart(runId: "run-a", plan: wire, hand: rig.host.hand).start()
        XCTAssertEqual(ReflexRuntimeHost.status(run: "run-a")["state"] as? String, "running", "the window confirms A")
        XCTAssertEqual(ReflexRuntimeHost.stop(run: "run-a", reason: StopReason.request)["state"] as? String, "stopped")
        _ = try HostStart(runId: "run-b", plan: wire, hand: rig.host.hand).start()
        let late = try XCTUnwrap(unlockedRoad("reflexStop", params: ["run": .string("run-a")]))
        XCTAssertEqual(late["runId"] as? String, "run-a", "the late stop answers A")
        XCTAssertEqual(late["state"] as? String, "stopped")
        let lateStatus = try XCTUnwrap(unlockedRoad("reflexStatus", params: ["run": .string("run-a")]))
        XCTAssertEqual(lateStatus["runId"] as? String, "run-a", "the late status reads A, never B")
        XCTAssertEqual(ReflexRuntimeHost.status(run: "run-b")["state"] as? String, "running", "B stands")
        XCTAssertEqual(rig.host.hand.snapshot.holder, .reflex("run-b"), "B still holds the hand")
        XCTAssertEqual(try unlockedRoad("reflexStatus", params: ["run": .string("never")])?["state"] as? String, ReflexRuntimeHost.missing)
        XCTAssertThrowsError(try unlockedRoad("reflexStop", params: [:]), "a reflex stop names its run") { error in
            XCTAssertEqual((error as? ProviderError)?.code, "invalid_argument")
        }
        XCTAssertEqual(ReflexRuntimeHost.status(run: "run-b")["state"] as? String, "running")
        OperatorGuardHost.stop(reason: StopReason.request)
        XCTAssertEqual(ReflexRuntimeHost.status(run: "run-b")["state"] as? String, "stopped", "the operator's stop ends whatever stands")
        XCTAssertNil(rig.host.hand.snapshot.holder)
    }

    /// A start asked again — its answer lost on the way — answers the run it started
    /// and starts nothing: while that run starts, while it stands and after it ended.
    /// Another start while one stands is refused as busy.
    func test_a_retried_start_starts_nothing_new() throws {
        let rig = HostRig()
        defer { rig.restore() }
        let gated = GatedKernel()
        ReflexRuntimeHost.kernel = gated
        gated.gate.close()
        let wire = try HostRig.clickWire()
        let first = Box<[String: Any]>()
        Thread.detachNewThread {
            first.value = try? HostStart(runId: "once", plan: wire, hand: rig.host.hand).start()
        }
        XCTAssertTrue(gated.gate.awaitArrival(), "the first start is opening its kernel session")
        var whileStarting: [String: Any] = [:]
        XCTAssertNoThrow(whileStarting = try HostStart(runId: "once", plan: wire, hand: rig.host.hand).start(),
                         "a start asked again while it starts answers that start")
        XCTAssertEqual(whileStarting["state"] as? String, "starting")
        XCTAssertEqual(whileStarting["retried"] as? Bool, true)
        gated.gate.open()
        XCTAssertTrue(eventually { first.value != nil })
        XCTAssertEqual(first.value?["state"] as? String, "running")
        var whileRunning: [String: Any] = [:]
        XCTAssertNoThrow(whileRunning = try HostStart(runId: "once", plan: wire, hand: rig.host.hand).start(),
                         "a retried start answers the run it started")
        XCTAssertEqual(whileRunning["retried"] as? Bool, true)
        XCTAssertEqual(whileRunning["planEpoch"] as? UInt64, first.value?["planEpoch"] as? UInt64, "the same run")
        XCTAssertThrowsError(try HostStart(runId: "other", plan: wire, hand: rig.host.hand).start()) { error in
            XCTAssertEqual((error as? ProviderError)?.code, "hand_busy")
        }
        _ = ReflexRuntimeHost.stop(run: "once", reason: StopReason.request)
        var afterEnd: [String: Any] = [:]
        XCTAssertNoThrow(afterEnd = try HostStart(runId: "once", plan: wire, hand: rig.host.hand).start())
        XCTAssertEqual(afterEnd["state"] as? String, "stopped", "an ended run answers its end, and nothing starts")
        XCTAssertEqual(afterEnd["retried"] as? Bool, true)
        XCTAssertNil(rig.host.hand.snapshot.holder, "nothing holds the hand")
        XCTAssertEqual(gated.scene.sessions, 1, "one kernel session for one run, however often it was asked")
    }

    /// A start answers while the run holds the hand: the start has returned, and
    /// the helper's one hand is still the run's — a request cannot take it, an
    /// acting verb is refused before it posts — until the run is stopped.
    func test_one_hand_remains_owned_after_start_returns() throws {
        let rig = HostRig()
        defer { rig.restore() }
        let answer = try HostStart(runId: "holding", plan: try HostRig.clickWire(), hand: rig.host.hand).start()
        XCTAssertEqual(answer["state"] as? String, "running", "the start has returned")
        XCTAssertEqual(rig.host.hand.snapshot.holder, .reflex("holding"), "the run holds the hand after it")
        XCTAssertThrowsError(try rig.host.hand.acquire(.request)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .busy(.reflex("holding")))
        }
        // The frames it reads are the run's, and its status names their scene.
        rig.show(.ball(track: 5, box: LiveRig.box))
        let scene = try XCTUnwrap(ReflexRuntimeHost.status(run: "holding")["scene"] as? [String: UInt64])
        XCTAssertEqual(scene["stream"], 1)
        XCTAssertEqual(scene["plan"], answer["planEpoch"] as? UInt64)
        XCTAssertNotNil(scene["owner"])
        _ = ReflexRuntimeHost.stop(run: "holding", reason: StopReason.request)
        XCTAssertNil(rig.host.hand.snapshot.holder, "the stop gives it back")
    }

    // MARK: 경계3 — receipts leave only when the window acknowledges them

    /// The collector reads a run's receipts after the last number it holds, as often as
    /// it asks — a read takes nothing — and an acknowledgement lets go of those it
    /// names. A run that ended keeps its last receipts until they are acknowledged, and
    /// a status never carries or takes them.
    func test_receipts_are_read_until_acknowledged_and_kept_after_the_run_ends() throws {
        let rig = HostRig()
        defer { rig.restore() }
        _ = try HostStart(runId: "kept", plan: try HostRig.clickWire(), hand: rig.host.hand).start()
        rig.show(.ball(track: 5, box: LiveRig.box))
        XCTAssertTrue(eventually { (ReflexRuntimeHost.status(run: "kept")["receiptsIssued"] as? UInt64) == 1 })
        for _ in 0..<3 { _ = ReflexRuntimeHost.status(run: "kept") }
        XCTAssertEqual(ReflexRuntimeHost.status(run: "kept")["receiptsPending"] as? Int, 1, "a status takes nothing")
        let read = ReflexRuntimeHost.receipts(run: "kept", after: 0)["receipts"] as? [[String: Any]]
        XCTAssertEqual(read?.map { $0["seq"] as? UInt64 }, [1])
        let again = ReflexRuntimeHost.receipts(run: "kept", after: 0)["receipts"] as? [[String: Any]]
        XCTAssertEqual(again?.count, 1, "an answer lost on the way is read again")
        _ = ReflexRuntimeHost.stop(run: "kept", reason: StopReason.request)
        let ended = ReflexRuntimeHost.receipts(run: "kept", after: 0)
        XCTAssertEqual(ended["state"] as? String, "stopped")
        XCTAssertEqual((ended["receipts"] as? [[String: Any]])?.count, 1, "the ended run's last receipts wait for the window")
        let acknowledged = ReflexRuntimeHost.acknowledge(run: "kept", through: 1)
        XCTAssertEqual(acknowledged["receiptsPending"] as? Int, 0)
        XCTAssertEqual(acknowledged["receiptsAcknowledged"] as? UInt64, 1)
        XCTAssertEqual((ReflexRuntimeHost.receipts(run: "kept", after: 1)["receipts"] as? [[String: Any]])?.count, 0)
        XCTAssertNil(ReflexRuntimeHost.status(run: "kept")["receipts"])
        XCTAssertEqual(ReflexRuntimeHost.receipts(run: "unknown", after: 0)["state"] as? String, ReflexRuntimeHost.missing)
    }

    /// A receipt as the window's collector reads it keeps every key it had and adds its leaf's
    /// detector's pick and the track its target was decided on (t-10223 R8): additions only, so
    /// a reader of the old keys reads them unchanged.
    func test_a_receipt_adds_its_pick_and_track_beside_its_keys() throws {
        let rig = HostRig()
        defer { rig.restore() }
        _ = try HostStart(runId: "picked", plan: try HostRig.clickWire(), hand: rig.host.hand).start()
        defer { _ = ReflexRuntimeHost.stop(run: "picked", reason: StopReason.request) }
        rig.show(.ball(track: 5, box: LiveRig.box))
        XCTAssertTrue(eventually { (ReflexRuntimeHost.status(run: "picked")["receiptsIssued"] as? UInt64) == 1 })
        let receipt = try XCTUnwrap((ReflexRuntimeHost.receipts(run: "picked", after: 0)["receipts"] as? [[String: Any]])?.first)
        let before: Set<String> = [
            "seq", "ruleId", "actionId", "leafIndex", "outcome", "targetId", "sourceCapture", "decidedHostNs",
            "decidedDeliveredHostNs", "admittedHostNs", "captureWaitNs", "firstEventHostNs", "firstEventFrameHostNs",
            "firstEventFrameDeliveredHostNs", "downHostNs", "upHostNs", "endedHostNs", "events",
        ]
        XCTAssertEqual(Set(receipt.keys), before.union(["pick", "trackId"]))
        XCTAssertEqual(receipt["pick"] as? String, "first")
        XCTAssertEqual(receipt["trackId"] as? UInt64, 5)
        XCTAssertEqual(receipt["targetId"] as? String, "ball#5")
    }

    /// A reader that stopped acknowledging lets the queue fill: the next leaf ends the
    /// run as `overflow` before it starts — nothing acts with nowhere to keep what it
    /// did — and the run lets go of the hand. Room made afterwards does not bring the
    /// run back, and a new edge fires nothing.
    func test_full_unacked_queue_ends_run_but_releases() throws {
        // A table whose queue holds four receipts: a glide of two waypoints at most.
        let raw = try ReflexFixtures.limitsWire()
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: raw) as? [String: Any])
        object["max_expanded_actions"] = 4
        object["max_pointer_duration_ms"] = 16
        let small = try ReflexContract.decodeLimits(try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]))
        XCTAssertNoThrow(try ReflexTable.check(small))
        let plan = try ReflexFixtures.plan { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [["id": "click1", "kind": "click", "target": "ball"]]]]
            object["pointer"] = ["curve": "linear", "duration_ms": 16, "instant": false]
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["max_fires"] = 2
            rules[0]["cooldown_ms"] = 0
            rules[0]["predicate"] = ["op": "eq", "value": 1]
            object["rules"] = rules
        }
        let rig = try LiveRig(plan: plan, policy: try ReflexFixtures.policy(renew: true), limits: small)
        rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: rig.track, box: LiveRig.box)) }
        try rig.session.start()
        for fire in 1...5 {
            rig.track = UInt64(4 + fire)
            rig.awayAndBack(waiting: fire <= 4)
            if fire <= 4 {
                XCTAssertTrue(eventually { rig.session.receipts.pending == fire }, "fire \(fire): its receipt kept, unacknowledged")
            }
        }
        XCTAssertTrue(eventually { rig.session.status.state == .stopped(ReflexRunReason.overflow) }, "\(rig.session.status.state)")
        XCTAssertEqual(rig.session.status.leaves, 4, "the fifth leaf never started")
        XCTAssertEqual(rig.downs, 4)
        XCTAssertEqual(rig.ups, 4, "every press let go of")
        XCTAssertNil(rig.hand.snapshot.holder, "the run gave the hand back")
        rig.session.receipts.acknowledge(through: 4)
        XCTAssertEqual(rig.session.receipts.pending, 0)
        rig.track = 20
        rig.awayAndBack(waiting: false)
        XCTAssertFalse(eventually(within: 0.3) { rig.downs > 4 }, "room made later does not bring the run back")
        XCTAssertEqual(rig.session.status.state, .stopped(ReflexRunReason.overflow))
    }
}
