import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class WindowListTests: XCTestCase {
    private static let app: pid_t = 4321
    private static let other: pid_t = 9876

    /// One window as the window server lists it; `name` and `sharing` absent
    /// are the keys the list leaves out.
    private func serverWindow(
        pid: pid_t = WindowListTests.app, number: Int, bounds: CGRect, name: String? = nil, sharing: Int? = 1, alpha: CGFloat = 1
    ) -> [String: Any] {
        var info: [String: Any] = [
            kCGWindowOwnerPID as String: pid,
            kCGWindowNumber as String: NSNumber(value: number),
            kCGWindowLayer as String: 0,
            kCGWindowBounds as String: bounds.dictionaryRepresentation,
            kCGWindowAlpha as String: alpha,
            kCGWindowIsOnscreen as String: true,
        ]
        if let name { info[kCGWindowName as String] = name }
        if let sharing { info[kCGWindowSharingState as String] = NSNumber(value: sharing) }
        return info
    }

    private let notes = CGRect(x: 100, y: 80, width: 900, height: 600)

    /// The window server's list as the helper always read it: the app's own
    /// windows that are big enough, seen and shared, the best first.
    func test_the_window_servers_list_is_read_for_one_app_as_before() {
        let infos = [
            serverWindow(number: 1, bounds: CGRect(x: 0, y: 0, width: 300, height: 200), name: "small"),
            serverWindow(number: 2, bounds: notes, name: "Notes"),
            serverWindow(number: 3, bounds: CGRect(x: 0, y: 0, width: 40, height: 40), name: "too small"),
            serverWindow(number: 4, bounds: notes, name: "hidden", alpha: 0),
            serverWindow(number: 5, bounds: notes, name: "not shared", sharing: 0),
            serverWindow(pid: Self.other, number: 6, bounds: notes, name: "another app's"),
        ]
        XCTAssertEqual(WindowList.candidates(infos: infos, pid: Self.app).map(\.windowId), [2, 1])
    }

    /// Without Screen Recording the window server names no other app's window
    /// and may list none of its windows as shared (t-66753 C1): a read of the
    /// app — its tree, never a picture — then looks at the app's own
    /// accessibility windows, and only at those: the minimized, the too
    /// small, the frameless and any window of another process stay out.
    func test_an_app_the_window_server_lists_no_window_of_is_read_through_its_own_accessibility_windows() {
        let listed = WindowList.candidates(
            infos: [serverWindow(number: 2, bounds: notes, sharing: 0)],
            pid: Self.app
        )
        XCTAssertEqual(listed, [], "the list as it reads without Screen Recording keeps no window of the app")
        var asked = 0
        let read = WindowList.forReading(listed: listed, pid: Self.app) {
            asked += 1
            return [
                AccessibilityWindow(pid: Self.app, number: 77, frame: notes, title: "Notes", minimized: false),
                AccessibilityWindow(pid: Self.app, number: 78, frame: notes, title: "Docked", minimized: true),
                AccessibilityWindow(pid: Self.app, number: 79, frame: CGRect(x: 0, y: 0, width: 30, height: 30), title: "Tiny", minimized: false),
                AccessibilityWindow(pid: Self.app, number: 80, frame: nil, title: "No frame", minimized: false),
                AccessibilityWindow(pid: Self.other, number: 81, frame: notes, title: "Another app's", minimized: false),
            ]
        }
        XCTAssertEqual(asked, 1, "the app's own windows are asked for once")
        XCTAssertEqual(read.map(\.windowId), [77], "the app's own accessibility window stands in for a read")
        XCTAssertEqual(read.first?.bounds, notes)
        XCTAssertEqual(read.first?.title, "Notes")
    }

    /// A window the window server lists is the one read: the accessibility
    /// windows are not even asked for (t-66753 C1).
    func test_a_listed_window_is_read_as_listed_and_the_tree_is_not_asked_for_windows() {
        let listed = WindowList.candidates(infos: [serverWindow(number: 2, bounds: notes, name: "Notes")], pid: Self.app)
        var asked = 0
        let read = WindowList.forReading(listed: listed, pid: Self.app) {
            asked += 1
            return [AccessibilityWindow(pid: Self.app, number: 77, frame: notes, title: "Notes", minimized: false)]
        }
        XCTAssertEqual(read, listed)
        XCTAssertEqual(asked, 0)
    }
}
