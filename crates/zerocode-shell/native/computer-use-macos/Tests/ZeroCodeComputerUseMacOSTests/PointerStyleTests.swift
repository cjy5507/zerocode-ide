import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class PointerStyleTests: XCTestCase {
    /// The window's reflex table, as the shared fixture holds it.
    private func table() throws -> ReflexLimits {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("zerocode-core/fixtures/reflex-contract/limits.json")
        return try ReflexContract.decodeLimits(Data(try Data(contentsOf: url).dropLast()))
    }

    private func style(_ durationMs: UInt64, _ curve: ReflexPointerCurve, instant: Bool = false) throws -> PointerStyle {
        let wire = #"{"curve":"\#(curve.rawValue)","duration_ms":\#(durationMs),"instant":\#(instant)}"#
        return try PointerStyle(try JSONDecoder().decode(ReflexPointerStyle.self, from: Data(wire.utf8)), limits: try table())
    }

    func testAGlideEndsExactlyOnItsTargetAtItsDurationOneWaypointPerTick() throws {
        let limits = try table()
        let glide = PointerSchedule.glide(
            from: .init(x: 0, y: 0), to: .init(x: 80, y: 40),
            style: try style(80, .cosine), tickNs: limits.pointer_tick_ns, startNs: 1_000)
        XCTAssertEqual(glide.count, 10, "80 ms at the table's 8 ms tick")
        XCTAssertEqual(glide.last, PointerWaypoint(x: 80, y: 40, dueNs: 1_000 + 80_000_000))
        XCTAssertEqual(glide.first?.dueNs, 1_000 + 8_000_000)
        for (earlier, later) in zip(glide, glide.dropFirst()) {
            XCTAssertLessThan(earlier.dueNs, later.dueNs)
            XCTAssertLessThanOrEqual(earlier.x, later.x, "no backtracking")
        }
        // Eased: the first tick covers less than a straight tenth.
        XCTAssertLessThan(glide[0].x, 8)
    }

    func testAStraightGlideCoversEqualShares() throws {
        let glide = PointerSchedule.glide(
            from: .init(x: 0, y: 0), to: .init(x: 100, y: 0),
            style: try style(40, .linear), tickNs: 10_000_000, startNs: 0)
        XCTAssertEqual(glide.map(\.x), [25, 50, 75, 100])
        XCTAssertEqual(glide.map(\.dueNs), [10_000_000, 20_000_000, 30_000_000, 40_000_000])
    }

    func testAPointerAlreadyThereWaitsOutNoPath() throws {
        let glide = PointerSchedule.glide(
            from: .init(x: 12, y: 34), to: .init(x: 12, y: 34),
            style: try style(80, .cosine), tickNs: 8_000_000, startNs: 5)
        XCTAssertEqual(glide, [], "D = 0 posts nothing and waits for nothing")
    }

    func testAnInstantStyleIsOneWaypointDueAtOnce() throws {
        let limits = try table()
        let glide = PointerSchedule.glide(
            from: .init(x: 0, y: 0), to: .init(x: 300, y: 200),
            style: try style(limits.instant_duration_ms, .linear, instant: true), tickNs: limits.pointer_tick_ns, startNs: 77)
        XCTAssertEqual(glide, [PointerWaypoint(x: 300, y: 200, dueNs: 77)])
    }

    func testAStyleOutsideTheTableIsRefused() throws {
        let limits = try table()
        XCTAssertThrowsError(try style(0, .linear))
        XCTAssertThrowsError(try style(limits.max_pointer_duration_ms + 1, .linear))
        XCTAssertThrowsError(try style(limits.instant_duration_ms + 1, .linear, instant: true), "instant is only the table's instant")
        XCTAssertNoThrow(try style(limits.max_pointer_duration_ms, .cosine))
    }

    func testTodaysSteppedMoveIsTheBaselineFirstAtOnceThenOneStepApart() {
        let path = PointerSchedule.steps(from: .init(x: 0, y: 0), to: .init(x: 100, y: 0), count: 4, eased: false, stepNs: 8_000_000, startNs: 100)
        XCTAssertEqual(path.map(\.x), [25, 50, 75, 100])
        XCTAssertEqual(path.map(\.dueNs), [100, 8_000_100, 16_000_100, 24_000_100])
        let eased = PointerSchedule.steps(from: .init(x: 0, y: 0), to: .init(x: 100, y: 0), count: 10, eased: true, stepNs: 8_000_000, startNs: 0)
        XCTAssertEqual(eased.map(\.x), SmoothPointerPath.points(from: .init(x: 0, y: 0), to: .init(x: 100, y: 0), steps: 10).map(\.x))
        XCTAssertEqual(PointerSchedule.steps(from: .init(x: 1, y: 1), to: .init(x: 2, y: 2), count: 0, eased: true, stepNs: 8, startNs: 0),
                       [PointerWaypoint(x: 2, y: 2, dueNs: 0)], "under one step is the plain jump")
    }

    func testALateHandPostsTheNewestWaypointDueNotTheOnesItMissed() {
        let path = (1...5).map { PointerWaypoint(x: Double($0), y: 0, dueNs: UInt64($0) * 10) }
        XCTAssertNil(PointerSchedule.due(path, after: -1, nowNs: 9), "nothing is due before the first")
        XCTAssertEqual(PointerSchedule.due(path, after: -1, nowNs: 10), 0)
        XCTAssertEqual(PointerSchedule.due(path, after: 0, nowNs: 45), 3, "woken at 45 it jumps to the fourth, no burst of the second and third")
        XCTAssertNil(PointerSchedule.due(path, after: 3, nowNs: 45))
        XCTAssertEqual(PointerSchedule.due(path, after: 3, nowNs: 1_000), 4)
        XCTAssertNil(PointerSchedule.due(path, after: 4, nowNs: 1_000), "the last posted, nothing is left")
    }
}
