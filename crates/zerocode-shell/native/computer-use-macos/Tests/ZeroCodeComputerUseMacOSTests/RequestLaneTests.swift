import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class RequestLaneTests: XCTestCase {
    /// A running app answers to its words and to `pid:N`; nothing else runs.
    private func resolve(_ written: String) -> String? {
        switch written.lowercased() {
        case "iphone mirroring", "com.apple.screencontinuity": return "pid:501"
        case "notes": return "pid:77"
        default: return written.hasPrefix("pid:") ? written : nil
        }
    }

    private func lane(_ method: String, app: String? = nil, owner: Int32? = nil, borrow: String? = nil) -> String? {
        RequestLane.name(method: method, app: app, windowOwner: owner, borrowApp: borrow, resolve: resolve)
    }

    func testARequestThatNamesAnAppWaitsInThatAppsLane() {
        XCTAssertEqual(lane("typeText", app: "iPhone Mirroring"), "pid:501")
        XCTAssertEqual(lane("click", app: "com.apple.ScreenContinuity"), "pid:501")
        XCTAssertEqual(lane("click", app: "pid:77"), "pid:77")
        XCTAssertEqual(lane("click", app: "Not Running"), "app:not running")
        XCTAssertEqual(lane("windowAction", owner: 77), "pid:77")
    }

    /// A press borrowed for a mirrored phone's window waits behind the keys going
    /// into that phone, not in the desktop lane every app-less look shares.
    func testAPressBorrowedForOneAppsWindowWaitsInThatAppsLane() {
        XCTAssertEqual(lane("mouseClick", borrow: "iPhone Mirroring"), "pid:501")
        XCTAssertEqual(lane("mouseClick", borrow: "  "), RequestLane.desktop)
        XCTAssertEqual(lane("mouseClick"), RequestLane.desktop)
        XCTAssertEqual(lane("key"), RequestLane.desktop)
    }

    /// The reads that touch no app and no input wait in no lane; every other
    /// app-less request still shares the desktop's.
    func testReadsThatTouchNoAppAndNoInputWaitInNoLane() {
        for method in ["screenshotDesktop", "displays", "cursorPosition", "listAllWindows"] {
            XCTAssertNil(lane(method), method)
        }
        // The clipboard is shared with the person and the other agents' pastes.
        XCTAssertEqual(lane("clipboardRead"), RequestLane.desktop)
        XCTAssertEqual(lane("readText"), RequestLane.desktop)
        // A read that names an app still waits in that app's lane.
        XCTAssertEqual(lane("screenshotDesktop", app: "Notes"), "pid:77")
    }
}
