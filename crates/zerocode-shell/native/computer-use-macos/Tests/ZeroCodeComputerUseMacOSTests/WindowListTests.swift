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

    /// An accessibility window is on the screen only as something said so
    /// (t-68324 ③): the app is hidden → off the screen; the window server
    /// said this window's number is on / off the screen → that; nothing said
    /// → unknown, never "on the screen". A window of another Space is the
    /// server's `off`, a window of a hidden app is the app's.
    func test_an_accessibility_window_is_on_the_screen_only_as_something_said_so() {
        let read = WindowList.forReading(listed: [], pid: Self.app) {
            [
                AccessibilityWindow(pid: Self.app, number: 1, frame: notes, title: "front", minimized: false, serverOnScreen: true),
                AccessibilityWindow(pid: Self.app, number: 2, frame: notes, title: "other Space", minimized: false, serverOnScreen: false),
                AccessibilityWindow(pid: Self.app, number: 3, frame: notes, title: "no word", minimized: false),
                AccessibilityWindow(pid: Self.app, number: 4, frame: notes, title: "app hidden", minimized: false, appHidden: true, serverOnScreen: true),
            ]
        }
        let presence = Dictionary(uniqueKeysWithValues: read.map { ($0.windowId, $0.presence) })
        XCTAssertEqual(presence[1], .onScreen, "the server said it is on the screen")
        XCTAssertEqual(presence[2], .offScreen, "the server said it is not: another Space")
        XCTAssertEqual(presence[3], .unknown, "nothing said where it is")
        XCTAssertEqual(presence[4], .offScreen, "a hidden app shows no window, whatever else was said")
        XCTAssertEqual(read.filter(\.isOnScreen).map(\.windowId), [1], "only the window known to be on the screen counts as on it")
    }

    /// A window known to be on the screen is read before one that is not, and
    /// before one nobody knows about, of the same size and title (t-68324 ③).
    func test_a_window_known_on_the_screen_ranks_before_an_unknown_one() {
        let read = WindowList.forReading(listed: [], pid: Self.app) {
            [
                AccessibilityWindow(pid: Self.app, number: 3, frame: notes, title: "Notes", minimized: false),
                AccessibilityWindow(pid: Self.app, number: 2, frame: notes, title: "Notes", minimized: false, serverOnScreen: false),
                AccessibilityWindow(pid: Self.app, number: 1, frame: notes, title: "Notes", minimized: false, serverOnScreen: true),
            ]
        }
        XCTAssertEqual(read.first?.windowId, 1, "the window known to be on the screen is read first")
    }

    /// The words an answer through the tree adds about where its window is
    /// (t-68324 ③): each state says its own fact, and an unknown one says it
    /// is not known rather than that the window is shown.
    func test_the_answer_says_where_the_window_is_in_words_that_tell_the_three_apart() {
        let said = [WindowPresence.onScreen, .offScreen, .unknown].map { WindowPresence.sentence(for: $0) }
        XCTAssertEqual(Set(said).count, 3, "three states, three sentences: \(said)")
        XCTAssertTrue(said[1].contains("not on the screen"), said[1])
        XCTAssertTrue(said[2].contains("not known"), said[2])
        XCTAssertFalse(said[2].contains("is on the screen"), "an unknown window is not called shown: \(said[2])")
    }

    /// What the window server says of an app's windows by number (t-68324
    /// ③): on its on-screen list → on the screen; only in its full list →
    /// not on it; in neither → nothing said. Another app's windows are not
    /// read at all.
    func test_the_window_servers_two_lists_say_which_of_an_apps_windows_are_on_the_screen() {
        let front = serverWindow(number: 1, bounds: notes)
        let otherSpace = serverWindow(number: 2, bounds: notes)
        let another = serverWindow(pid: Self.other, number: 3, bounds: notes)
        let view = WindowList.serverView(onScreen: [front, another], all: [front, otherSpace, another], pid: Self.app)
        XCTAssertEqual(view, [1: true, 2: false])
        XCTAssertNil(view[3], "another app's window is not read")
        XCTAssertNil(view[4], "a number it lists nowhere has no word")
    }
}
