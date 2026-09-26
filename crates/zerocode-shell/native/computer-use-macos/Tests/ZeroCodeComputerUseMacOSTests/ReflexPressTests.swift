import CoreGraphics
import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOS
@testable import ZeroCodeComputerUseMacOSCore

/// A boundary that allows every point and names keys' route from `routes`, one
/// answer an ask — the last one again once they run out.
final class KeyRouting: ReflexInputBoundary, @unchecked Sendable {
    private let lock = NSLock()
    private let routes: [ReflexKeyRoute]
    private var asks = 0

    init(_ routes: ReflexKeyRoute...) { self.routes = routes }

    func refusal(_ input: ReflexLeaseInput, at point: SmoothPointerPath.Point) -> String? { nil }

    func keyRoute() -> ReflexKeyRoute {
        lock.lock()
        defer { lock.unlock() }
        let route = routes[min(asks, routes.count - 1)]
        asks += 1
        return route
    }

    var asked: Int {
        lock.lock()
        defer { lock.unlock() }
        return asks
    }
}

extension LeafRig {
    /// The rig's runner with `boundary` in place of one that asks nothing.
    func runner(boundary: any ReflexInputBoundary) throws -> ReflexLeafRunner {
        let plan = try ReflexFixtures.plan()
        let admissions = self.admissions
        return ReflexLeafRunner(
            runId: "run", hand: hand, token: token, limits: limits,
            style: try PointerStyle(plan.plan.pointer, limits: limits),
            sightings: sightings, admit: { admissions.admit() },
            fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
            boundary: boundary,
            deadlineNs: .max
        )
    }

    /// Every sleep of the leaf shows the ball on a newer capture, of track
    /// `track(index)` for the `index`th sleep.
    func showBall(track: @escaping (Int) -> UInt64 = { _ in 5 }) {
        var seq: UInt64 = 10
        sleeper.onSleep = { index, _ in
            seq += 1
            self.capture(seq, track: track(index))
        }
    }
}

final class ReflexPressTests: XCTestCase {
    private let control = CGEventFlags.maskControl.rawValue
    private let shift = CGEventFlags.maskShift.rawValue

    /// ctrl+1 as the helper's key table codes it.
    private var ctrlOne: KeyChordStroke { KeyMapKeyboard().chord(key: "1", modifiers: ["ctrl"])! }

    private func keyLeaf(_ chord: KeyChordStroke?) -> ReflexLeaf {
        ReflexLeaf(ruleId: "assign", actionId: "key1", kind: .key, detector: "ball",
                   press: ReflexPress(chord: chord, key: "1", modifiers: ["ctrl"]))
    }

    // MARK: keys

    /// A key goes to the process of the run's app and nowhere else: its chord
    /// — the modifier down, the key down and up under it, the modifier up — on
    /// one admission, once a capture newer than the one that decided it still
    /// shows its target; the receipt names the key and what it held.
    func test_a_key_leaf_posts_its_chord_to_the_run_apps_process_alone() throws {
        let rig = try LeafRig()
        rig.decide()
        rig.showBall()
        let routing = KeyRouting(.process(4_242))
        let receipt = try rig.runner(boundary: routing).run(keyLeaf(ctrlOne), index: 0)
        XCTAssertEqual(receipt.outcome, .done)
        XCTAssertEqual(rig.admissions.admitted, 1)
        XCTAssertEqual(rig.poster.events.map(\.event.kind), [
            .key(code: 59, down: true, modifier: control), .key(code: 18, down: true, modifier: 0),
            .key(code: 18, down: false, modifier: 0), .key(code: 59, down: false, modifier: 0),
        ])
        XCTAssertEqual(rig.poster.events.map(\.event.flags), [control, control, control, 0], "the key under its modifier, the modifier up last")
        XCTAssertEqual(Set(rig.poster.events.map(\.event.route)), [.process(4_242)], "never the desktop")
        XCTAssertEqual(rig.hand.snapshot.held, [], "every press let go")
        XCTAssertEqual(routing.asked, 2, "asked before the lease and again just before the press")
        XCTAssertEqual(receipt.kind, .key)
        XCTAssertEqual(receipt.key, "1")
        XCTAssertEqual(receipt.modifiers, ["ctrl"])
        XCTAssertNil(receipt.button)
        XCTAssertEqual(receipt.events, 4)
        XCTAssertNotNil(receipt.downHostNs)
        XCTAssertNotNil(receipt.upHostNs)
        XCTAssertGreaterThan(receipt.firstEventFrameHostNs ?? 0, receipt.decidedHostNs ?? .max, "the newer capture let it go")
    }

    /// No route, no key: an app that is gone — before the lease, or by the
    /// press — ends the leaf as `scope` with the boundary's words, nothing
    /// posted.
    func test_a_key_goes_nowhere_once_the_run_app_is_gone() throws {
        for routes in [[ReflexKeyRoute.refused("the app is gone")],
                       [.process(4_242), .refused("the app is gone")]] {
            let rig = try LeafRig()
            rig.decide()
            rig.showBall()
            let routing = routes.count == 1 ? KeyRouting(routes[0]) : KeyRouting(routes[0], routes[1])
            let receipt = try rig.runner(boundary: routing).run(keyLeaf(ctrlOne), index: 0)
            XCTAssertEqual(receipt.outcome, .scope, "\(routes)")
            XCTAssertEqual(receipt.reason, "the app is gone")
            XCTAssertEqual(rig.poster.events.count, 0, "nothing pressed")
            XCTAssertEqual(rig.admissions.admitted, 1)
        }
        // A boundary that names no route for keys sends them nowhere.
        let rig = try LeafRig()
        rig.decide()
        rig.showBall()
        let receipt = try rig.runner().run(keyLeaf(ctrlOne), index: 0)
        XCTAssertEqual(receipt.outcome, .scope)
        XCTAssertEqual(rig.poster.events.count, 0)
    }

    /// A chord the platform stops midway lets go of every modifier it pressed:
    /// the key's own press refused, the modifier that went down comes up, and
    /// nothing is left held.
    func test_a_chord_cut_midway_lets_go_of_its_modifiers() throws {
        let rig = try LeafRig()
        rig.decide()
        rig.showBall()
        rig.poster.refuse = { event in event.kind == .key(code: 18, down: true, modifier: 0) }
        let receipt = try rig.runner(boundary: KeyRouting(.process(4_242))).run(keyLeaf(ctrlOne), index: 0)
        XCTAssertEqual(receipt.outcome, .failed)
        XCTAssertEqual(rig.poster.events.map(\.event.kind), [
            .key(code: 59, down: true, modifier: control), .key(code: 59, down: false, modifier: 0),
        ])
        XCTAssertEqual(rig.poster.events.last?.event.route, .process(4_242), "let go where it was pressed")
        XCTAssertEqual(rig.hand.snapshot.held, [])
        // A key nobody named a code for presses nothing.
        let uncoded = try LeafRig()
        uncoded.decide()
        uncoded.showBall()
        XCTAssertEqual(try uncoded.runner(boundary: KeyRouting(.process(4_242))).run(keyLeaf(nil), index: 0).outcome, .failed)
        XCTAssertEqual(uncoded.poster.events.count, 0)
    }

    // MARK: buttons and modifiers

    func test_an_unaimable_sliver_does_not_spend_the_edge_before_the_target_is_exposed() throws {
        let rig = try LiveRig(plan: ReflexFixtures.clickPlan(maxFires: 1))
        defer { rig.session.stop(reason: StopReason.request) }
        rig.sleeper.onSleep = { _, _ in rig.frame(.ball(track: 5, box: LiveRig.box)) }
        try rig.session.start()
        rig.frame(.ball(track: 5, box: ReflexRoi(x: 8, y: 8, width: 1, height: 16, space: .pixel)))
        XCTAssertEqual(rig.session.status.fires, 0, "a detected sliver has no safe aim yet")
        rig.frame(.ball(track: 5, box: LiveRig.box))
        XCTAssertEqual(rig.receipts(1).first?.outcome, .done, "the same true edge fires once when its target can be aimed at")
        XCTAssertEqual(rig.downs, 1)
        XCTAssertEqual(rig.session.status.fires, 1)
    }

    func test_a_click_aims_at_the_same_tracks_latest_position_after_its_glide() throws {
        let rig = try LeafRig()
        rig.decide()
        rig.showBall()
        let moved = ReflexRoi(x: 24, y: 8, width: 16, height: 16, space: .pixel)
        rig.poster.onPost = { event, _ in
            if event.kind == .pointerMove, rig.poster.moves.count == 10 {
                rig.capture(90, box: moved)
            }
        }
        let receipt = try rig.runner().run(LeafRig.click, index: 0)
        XCTAssertEqual(receipt.outcome, .done)
        XCTAssertEqual(rig.poster.presses.map { SmoothPointerPath.Point(x: $0.x, y: $0.y) },
                       [SmoothPointerPath.Point(x: 32, y: 16)])
        XCTAssertEqual(rig.poster.releases.map { SmoothPointerPath.Point(x: $0.x, y: $0.y) },
                       [SmoothPointerPath.Point(x: 32, y: 16)])
    }

    func test_the_aim_comparison_uses_the_frame_position_or_its_measured_velocity() throws {
        for (method, expectedX) in [(ReflexPressAim.latest, 16.0), (.predicted, 26.0), (.resting, 16.0)] {
            let rig = try LeafRig()
            rig.decide()
            rig.showBall()
            rig.poster.onPost = { event, _ in
                if event.kind == .pointerMove, rig.poster.moves.count == 10 {
                    let frame = ReflexFixtures.frame(capture: 90, capturedNs: rig.clock.nowNs() - 10_000_000,
                                                     deliveredNs: rig.clock.nowNs(), owner: rig.token.id)
                    let observed = ReflexFixtures.ball(on: frame, track: 5, box: rig.box)
                    let moving = ReflexObservation(
                        detector_id: observed.detector_id, frame: observed.frame, unknown: nil, value: 1,
                        target: ReflexTarget(track_id: 5, roi: rig.box, point_x: 16, point_y: 16,
                                             velocity_x: 1_000, velocity_y: 0, uncertainty: 1),
                        scale: observed.scale, cells: nil, samples: observed.samples
                    )
                    rig.sightings.publish(ReflexFixtures.seen(frame, moving))
                }
            }
            var runner = try rig.runner()
            runner.pressAim = method
            XCTAssertEqual(runner.run(LeafRig.click, index: 0).outcome, .done, method.rawValue)
            XCTAssertEqual(rig.poster.presses.first?.x, expectedX, method.rawValue)
        }
    }

    func test_a_click_already_inside_its_target_does_not_repeat_the_whole_glide() throws {
        let rig = try LeafRig()
        let target = ReflexRoi(x: 12, y: 8, width: 16, height: 16, space: .pixel)
        rig.capture(10, box: target)
        rig.poster.movePointer(to: SmoothPointerPath.Point(x: 16, y: 16))
        var seq: UInt64 = 10
        rig.sleeper.onSleep = { _, deadline in
            rig.clock.set(deadline)
            seq += 1
            rig.capture(seq, box: target)
        }
        XCTAssertEqual(try rig.runner().run(LeafRig.click, index: 0).outcome, .done)
        XCTAssertEqual(rig.poster.moves.count, 0, "a moving target under the pointer needs a current press point, not another full approach")
        XCTAssertEqual(rig.poster.presses.first?.x, 20)
    }

    func test_a_current_target_does_not_allow_a_click_when_the_pointer_cannot_be_read() throws {
        let rig = try LeafRig()
        rig.decide()
        rig.showBall()
        rig.poster.forgetPointer()
        XCTAssertEqual(try rig.runner().run(LeafRig.click, index: 0).outcome, .moved)
        XCTAssertTrue(rig.poster.presses.isEmpty)
    }

    /// A right click presses the right button; a shift click the left one with
    /// shift's flag on its press and its release — flags on the mouse events
    /// alone, never a key of the desktop's.
    func test_a_right_click_and_a_shift_click_press_their_button_under_their_flags() throws {
        let right = try LeafRig()
        right.decide()
        right.showBall()
        let rightLeaf = ReflexLeaf(ruleId: "order", actionId: "click1", kind: .click, detector: "ball", press: ReflexPress(button: .right))
        let ordered = try right.runner().run(rightLeaf, index: 0)
        XCTAssertEqual(ordered.outcome, .done)
        XCTAssertEqual(right.poster.presses.map(\.kind), [.buttonDown(.right, clickState: 1)])
        XCTAssertEqual(right.poster.releases.map(\.kind), [.buttonUp(.right, clickState: 1)])
        XCTAssertEqual(ordered.button, .right)

        let shifted = try LeafRig()
        shifted.decide()
        shifted.showBall()
        let shiftLeaf = ReflexLeaf(ruleId: "add", actionId: "click1", kind: .click, detector: "ball",
                                   press: ReflexPress(flags: shift, modifiers: ["shift"]))
        let added = try shifted.runner().run(shiftLeaf, index: 0)
        XCTAssertEqual(added.outcome, .done)
        XCTAssertEqual(shifted.poster.presses.map(\.kind), [.buttonDown(.left, clickState: 1)])
        XCTAssertEqual(shifted.poster.presses.map(\.flags), [shift])
        XCTAssertEqual(shifted.poster.releases.map(\.flags), [shift])
        XCTAssertTrue(shifted.poster.events.allSatisfy { if case .key = $0.event.kind { return false } else { return true } },
                      "no modifier key goes anywhere")
        XCTAssertEqual(added.modifiers, ["shift"])
        // Modifiers nobody resolved press nothing.
        let unresolved = try LeafRig()
        unresolved.decide()
        unresolved.showBall()
        let bare = ReflexLeaf(ruleId: "add", actionId: "click1", kind: .click, detector: "ball",
                              press: ReflexPress(flags: nil, modifiers: ["shift"]))
        XCTAssertEqual(try unresolved.runner().run(bare, index: 0).outcome, .failed)
        XCTAssertEqual(unresolved.poster.events.count, 0)
    }

    // MARK: drags

    private var box: ReflexLeaf {
        ReflexLeaf(ruleId: "select", actionId: "box1", kind: .drag, detector: "ball",
                   press: ReflexPress(from: ReflexDragPoint(x: -250, y: -250), to: ReflexDragPoint(x: 1_250, y: 1_250)))
    }

    /// A drag is one leaf on one admission: a glide to `from` on the target's
    /// hitbox, the left button down there, the move to `to` with it held at the
    /// pointer tick, and the release at `to` — no sooner than a click's fence
    /// after the press.
    func test_a_drag_glides_presses_moves_held_and_lets_go_as_one_leaf() throws {
        let rig = try LeafRig()
        rig.decide()
        rig.showBall()
        let receipt = try rig.runner().run(box, index: 0)
        XCTAssertEqual(receipt.outcome, .done)
        XCTAssertEqual(rig.admissions.admitted, 1)
        // The ball's hitbox is 8…24: a quarter of it outside each corner.
        let kinds = rig.poster.events.map(\.event.kind)
        let down = try XCTUnwrap(kinds.firstIndex(of: .buttonDown(.left, clickState: 1)))
        XCTAssertEqual(rig.poster.events[down].event.x, 4)
        XCTAssertEqual(rig.poster.events[down].event.y, 4)
        XCTAssertTrue(kinds[..<down].allSatisfy { $0 == .pointerMove }, "a glide to the press")
        let held = kinds[(down + 1)...].dropLast()
        XCTAssertFalse(held.isEmpty)
        XCTAssertTrue(held.allSatisfy { $0 == .buttonDrag(.left, clickState: 1) }, "moves with the button held")
        XCTAssertEqual(kinds.last, .buttonUp(.left, clickState: 1))
        XCTAssertEqual(rig.poster.events.last?.event.x, 28)
        XCTAssertEqual(rig.poster.events.last?.event.y, 28)
        XCTAssertEqual(rig.hand.snapshot.held, [])
        XCTAssertEqual(receipt.kind, .drag)
        XCTAssertEqual(receipt.button, .left)
        let pressed = try XCTUnwrap(receipt.downHostNs), released = try XCTUnwrap(receipt.upHostNs)
        XCTAssertGreaterThanOrEqual(released - pressed, UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000)
        XCTAssertEqual(receipt.events, UInt64(kinds.count))
    }

    /// Pressed, a drag whose target stops showing on a newer capture ends
    /// there and lets go of its button where the pointer stands: nothing is
    /// left held and nothing more moves.
    func test_a_drag_cut_after_its_press_lets_go_where_it_stands() throws {
        let rig = try LeafRig()
        rig.decide()
        // The glide to the press sleeps ten times; two held moves later the ball is another track.
        rig.showBall { $0 >= 12 ? 6 : 5 }
        let receipt = try rig.runner().run(box, index: 0)
        XCTAssertEqual(receipt.outcome, .moved)
        let kinds = rig.poster.events.map(\.event.kind)
        XCTAssertEqual(kinds.filter { $0 == .buttonDown(.left, clickState: 1) }.count, 1)
        XCTAssertEqual(kinds.last, .buttonUp(.left, clickState: 1), "let go, last")
        let lastHeld = try XCTUnwrap(rig.poster.events.last { $0.event.kind == .buttonDrag(.left, clickState: 1) })
        XCTAssertEqual(rig.poster.events.last?.event.x, lastHeld.event.x, "where the pointer stood")
        XCTAssertEqual(rig.hand.snapshot.held, [])
        // Stopped before it pressed: nothing to let go of, no press at all.
        let early = try LeafRig()
        early.decide()
        early.showBall { $0 >= 3 ? 6 : 5 }
        XCTAssertEqual(try early.runner().run(box, index: 0).outcome, .moved)
        XCTAssertTrue(early.poster.events.allSatisfy { $0.event.kind == .pointerMove })
    }

    // MARK: the helper's codes

    /// The key verbs and a reflex key post one sequence (`KeyChordStroke`): each
    /// modifier down with the flags held so far, the key under all of them, then
    /// the key up and each modifier up, newest first — the order `pressKey` has
    /// always posted.
    func test_the_key_verbs_and_a_reflex_key_post_one_sequence() throws {
        let stroke = try KeyMap.parse("cmd+shift+a").stroke
        let command = CGEventFlags.maskCommand.rawValue
        XCTAssertEqual(stroke.downs(route: .desktop).map(\.kind), [
            .key(code: 55, down: true, modifier: command), .key(code: 56, down: true, modifier: shift),
            .key(code: 0, down: true, modifier: 0),
        ])
        XCTAssertEqual(stroke.downs(route: .desktop).map(\.flags), [command, command | shift, command | shift])
        XCTAssertEqual(stroke.ups(route: .desktop).map(\.kind), [
            .key(code: 0, down: false, modifier: 0), .key(code: 56, down: false, modifier: 0),
            .key(code: 55, down: false, modifier: 0),
        ])
        XCTAssertEqual(stroke.ups(route: .desktop).map(\.flags), [command | shift, command, 0])
        XCTAssertEqual(Set(stroke.downs(route: .process(7)).map(\.route)), [.process(7)])
    }

    /// The helper's own key table codes every key and modifier the window's key
    /// table may send — the function keys and `opt` among them — so a plan that
    /// validates never meets a key the helper cannot press.
    func test_the_helpers_key_table_codes_every_word_the_window_may_send() throws {
        let table = try ReflexFixtures.keys()
        let keyboard = KeyMapKeyboard()
        for key in table.keys {
            XCTAssertNotNil(keyboard.chord(key: key, modifiers: []), key)
        }
        for modifier in table.modifiers {
            XCTAssertNotNil(keyboard.flags([modifier]), modifier)
        }
        XCTAssertEqual(keyboard.chord(key: "f5", modifiers: ["opt", "shift"]),
                       KeyChordStroke(key: 96, modifiers: [.init(code: 58, flag: CGEventFlags.maskAlternate.rawValue),
                                                           .init(code: 56, flag: shift)]))
        XCTAssertNil(keyboard.chord(key: "f13", modifiers: []))
        XCTAssertNil(keyboard.flags(["hyper"]))
    }

    /// A run's presses are named once when it starts: a key's chord and a
    /// click's or a drag's flags in the helper's codes, keyed by action; an
    /// action whose word the helper cannot code refuses the whole start.
    func test_a_runs_presses_are_resolved_once_when_it_starts() throws {
        let plan = try ReflexFixtures.plan { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [
                ["id": "key1", "kind": "key", "target": "ball", "key": "f5", "modifiers": ["opt", "shift"]],
                ["id": "click1", "kind": "click", "target": "ball", "modifiers": ["shift"]],
                ["id": "box1", "kind": "drag", "target": "ball", "from": ["x": 0, "y": 0], "to": ["x": 1_000, "y": 1_000]],
                ["id": "move1", "kind": "move", "target": "ball"],
            ]]]
        }
        let presses = try ReflexPresses.resolve(plan, keyboard: KeyMapKeyboard())
        XCTAssertEqual(Set(presses.keys), ["key1", "click1", "box1"], "a move presses nothing")
        XCTAssertEqual(presses["key1"]?.chord?.key, 96)
        XCTAssertEqual(presses["click1"]?.flags, shift)
        XCTAssertEqual(presses["box1"]?.flags, 0)
        XCTAssertEqual(presses["box1"]?.to, ReflexDragPoint(x: 1_000, y: 1_000))
        let leaves = ReflexMacros.leaves(ruleId: "follow", macroId: "tap", in: plan.plan, presses: presses)
        XCTAssertEqual(leaves.map(\.kind), [.key, .click, .drag, .move])
        XCTAssertEqual(leaves[0].press.chord, presses["key1"]?.chord)

        struct NoCodes: ReflexKeyboard {
            func chord(key: String, modifiers: [String]) -> KeyChordStroke? { nil }
            func flags(_ modifiers: [String]) -> UInt64? { nil }
        }
        XCTAssertThrowsError(try ReflexPresses.resolve(plan, keyboard: NoCodes())) { error in
            XCTAssertEqual(error as? ReflexPresses.Uncoded, ReflexPresses.Uncoded(action: "key1", words: ["opt", "shift", "f5"]))
        }
    }

    // MARK: the desktop's boundary

    /// A key goes to the process the run began with, and only while that very
    /// process runs: gone, or its pid taken by another process, it goes nowhere.
    func test_a_key_goes_only_to_the_process_the_run_began_with() {
        let started = Box<UInt64>()
        started.value = 111
        let boundary = DesktopRunBoundary(scope: ReflexActingScope(surface: .macos_desktop, target: "dev.zerocode.bench.rts", pid: 4_242),
                                          owner: { _ in 4_242 }, isOwn: { _ in false }, started: { _ in started.value })
        XCTAssertEqual(boundary.keyRoute(), .process(4_242))
        started.value = 222
        guard case let .refused(reason) = boundary.keyRoute() else { return XCTFail("a pid another process took") }
        XCTAssertTrue(reason.contains("dev.zerocode.bench.rts"), reason)
        started.value = nil
        XCTAssertNotEqual(boundary.keyRoute(), .process(4_242), "a process that no longer runs")
        XCTAssertEqual(ProcessStart.of(getpid()), ProcessStart.of(getpid()), "this process's start stands")
        XCTAssertNil(ProcessStart.of(0))
    }

    // MARK: through the helper's host

    #if !REFLEX_BENCH
    func test_a_production_helper_refuses_the_benchmark_aim_selector_before_taking_the_hand() throws {
        let rig = HostRig()
        defer { rig.restore() }
        XCTAssertThrowsError(try HostStart(runId: "bench-only", plan: ReflexFixtures.wire(), hand: rig.host.hand).start(benchPressAim: "resting")) { error in
            XCTAssertEqual((error as? ProviderError)?.code, "invalid_argument")
        }
        XCTAssertNil(rig.host.hand.snapshot.holder)
    }
    #endif

    /// A key plan started through the helper's host names its codes at the
    /// start and presses its key into the run's app's process on the ball's edge.
    func test_a_host_run_presses_its_key_into_the_run_apps_process() throws {
        let rig = HostRig()
        defer { rig.restore() }
        ReflexRuntimeHost.parts.boundary = { scope in KeyRouting(.process(scope.pid)) }
        // A key glides nowhere: the leaf's first wait is for the next capture, which a
        // display delivers a moment later — never at the instant its source was stamped.
        rig.host.sleeper.onSleep = { [weak rig] _, _ in
            guard let rig else { return }
            rig.host.clock.set(rig.host.clock.nowNs() + 1_000_000)
            rig.show(.ball(track: 5, box: LiveRig.box))
        }
        let wire = try ReflexFixtures.wire { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [["id": "key1", "kind": "key", "target": "ball", "key": "q"]]]]
            var rules = object["rules"] as! [[String: Any]]
            rules[0]["max_fires"] = 1
            rules[0]["cooldown_ms"] = 0
            rules[0]["predicate"] = ["op": "eq", "value": 1]
            object["rules"] = rules
        }
        _ = try HostStart(runId: "keyed", plan: wire, hand: rig.host.hand).start()
        defer { _ = ReflexRuntimeHost.stop(run: "keyed", reason: StopReason.request) }
        rig.show(.ball(track: 5, box: LiveRig.box))
        XCTAssertTrue(eventually { (ReflexRuntimeHost.status(run: "keyed")["receiptsIssued"] as? UInt64) == 1 })
        let keys = rig.host.poster.events.filter { if case .key = $0.event.kind { return true } else { return false } }
        XCTAssertEqual(keys.map(\.event.kind), [.key(code: 12, down: true, modifier: 0), .key(code: 12, down: false, modifier: 0)])
        XCTAssertEqual(Set(keys.map(\.event.route)), [.process(4_242)])
        let receipt = try XCTUnwrap((ReflexRuntimeHost.receipts(run: "keyed", after: 0)["receipts"] as? [[String: Any]])?.first)
        XCTAssertEqual(receipt["kind"] as? String, "key")
        XCTAssertEqual(receipt["key"] as? String, "q")
        XCTAssertEqual(receipt["outcome"] as? String, "done")
    }

    /// A start whose key the helper's own table cannot code is refused before
    /// the eye or the hand, naming the word.
    func test_a_start_refuses_a_key_its_helper_has_no_code_for() throws {
        let rig = HostRig()
        defer { rig.restore() }
        var table = try XCTUnwrap(JSONSerialization.jsonObject(with: ReflexFixtures.keysWire()) as? [String: Any])
        table["keys"] = (table["keys"] as? [String] ?? []) + ["f13"]
        let keys = try JSONSerialization.data(withJSONObject: table, options: [.sortedKeys, .withoutEscapingSlashes])
        let wire = try ReflexFixtures.wire { object in
            object["macros"] = [["id": "tap", "repeat": 1, "actions": [["id": "key1", "kind": "key", "target": "ball", "key": "f13"]]]]
        }
        XCTAssertThrowsError(try HostStart(runId: "uncoded", plan: wire, hand: rig.host.hand, keys: keys).start()) { error in
            XCTAssertEqual((error as? ProviderError)?.code, "unsupported_capability")
            XCTAssertTrue((error as? ProviderError)?.message.contains("f13") == true, "\(error)")
        }
        XCTAssertNil(rig.host.hand.snapshot.holder, "the hand was never taken")
        XCTAssertEqual(ReflexRuntimeHost.status(run: "uncoded")["state"] as? String, ReflexRuntimeHost.missing)
    }
}
