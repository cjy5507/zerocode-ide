import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// An act goes as its drawn pointer sets off; only the press ring waits for
/// the pointer to arrive (2026-11-15: the act's wait was 240 of 423 ms).
final class PointerArrivalTests: XCTestCase {
    private let point = CGPoint(x: 10, y: 20)

    func testAPointerThatSetsOffArrivesAfterItsMove() {
        let arrival = PointerArrival(point: point, pid: 41, setOffNs: 1_000, moveSeconds: 0.25)
        XCTAssertEqual(arrival.dueNs, 1_000 + 250_000_000)
        XCTAssertEqual(arrival.pid, 41)
        XCTAssertEqual(arrival.point, point)
    }

    func testAPointerAlreadyOnThePointIsThereAtOnce() {
        for seconds in [0, -1, Double.nan, Double.infinity] {
            XCTAssertEqual(PointerArrival(point: point, pid: 41, setOffNs: 7, moveSeconds: seconds).dueNs, 7, "\(seconds)")
        }
    }

    func testTheRingWaitsOnlyForWhatIsLeftOfTheMove() {
        let arrival = PointerArrival(point: point, pid: 41, dueNs: 500_000_000)
        XCTAssertEqual(arrival.secondsLeft(nowNs: 0), 0.5, accuracy: 1e-9)
        XCTAssertEqual(arrival.secondsLeft(nowNs: 420_000_000), 0.08, accuracy: 1e-9, "the press took 420 ms of the move")
        XCTAssertEqual(arrival.secondsLeft(nowNs: 500_000_000), 0)
        XCTAssertEqual(arrival.secondsLeft(nowNs: 900_000_000), 0, "a press slower than the move draws its ring at once")
    }
}
