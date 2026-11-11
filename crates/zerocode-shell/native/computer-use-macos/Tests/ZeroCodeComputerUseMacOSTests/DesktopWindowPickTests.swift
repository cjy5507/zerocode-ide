import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// Which accessibility window is the numbered one (t-15085), every window
/// written out: nothing here reads a screen.
final class DesktopWindowPickTests: XCTestCase {
    private typealias Row = (number: UInt32?, frame: CGRect?)
    private let target: UInt32 = 77

    private typealias Named = (number: UInt32?, frame: CGRect?, title: String?)

    private func pick(_ bounds: CGRect?, _ windows: [Row]) -> DesktopWindowPick.Pick {
        DesktopWindowPick.pick(
            target: target, targetBounds: bounds,
            windows: windows.map { (number: $0.number, frame: $0.frame, title: nil) })
    }

    private func pick(_ bounds: CGRect?, called title: String?, _ windows: [Named]) -> DesktopWindowPick.Pick {
        DesktopWindowPick.pick(target: target, targetBounds: bounds, targetTitle: title, windows: windows)
    }

    func testTheNumberIsFoundWhateverTheFrame() {
        let rows: [Row] = [(1, CGRect(x: 0, y: 0, width: 10, height: 10)), (77, CGRect(x: 500, y: 500, width: 9, height: 9))]
        XCTAssertEqual(pick(CGRect(x: 0, y: 0, width: 10, height: 10), rows), .found(1))
    }

    func testWithNoNumberExactlyOneFrameThatFitsIsUsedOnEveryKindOfDesk() {
        // Origin, a screen left of the main one, above it, and a Retina
        // screen's point frame: all the window list's global top-left points.
        for frame in [
            CGRect(x: 0, y: 0, width: 720, height: 440), CGRect(x: -1920, y: 0, width: 720, height: 440),
            CGRect(x: 100, y: -982, width: 720, height: 440), CGRect(x: 3008, y: 40, width: 3008, height: 1692),
        ] {
            let rows: [Row] = [(nil, CGRect(x: 9_999, y: 9_999, width: 5, height: 5)), (nil, frame)]
            XCTAssertEqual(pick(frame, rows), .found(1), "\(frame)")
        }
    }

    func testTheToleranceIsTwoPointsInclusive() {
        let bounds = CGRect(x: 100, y: 100, width: 600, height: 400)
        XCTAssertEqual(DesktopWindowPick.frameTolerance, 2)
        XCTAssertEqual(pick(bounds, [(nil, CGRect(x: 102, y: 98, width: 602, height: 398))]), .found(0))
        XCTAssertEqual(pick(bounds, [(nil, CGRect(x: 102.5, y: 100, width: 600, height: 400))]), .none)
        XCTAssertEqual(pick(bounds, [(nil, CGRect(x: 100, y: 100, width: 600, height: 402.5))]), .none)
    }

    func testTwoNumberlessWindowsAtTheFrameAreRefusedNotGuessed() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        XCTAssertEqual(pick(frame, [(nil, frame), (nil, frame.offsetBy(dx: 1, dy: 0))]), .ambiguous)
    }

    /// Two full-size windows of one app, stacked on a screen left of the main
    /// one (2026-11-11): the app publishes no number, the frames are the same,
    /// and the window list calls the target by its name.
    func testTwoNumberlessWindowsAtOneFrameAreToldApartByWhatTheyAreCalled() {
        let frame = CGRect(x: -4480, y: -68, width: 1920, height: 1050)
        let rows: [Named] = [
            (nil, frame, "Inbox - Browser"), (nil, frame, "Docs - Browser"),
            (nil, CGRect(x: 0, y: 33, width: 1512, height: 949), "Docs - Browser"),
        ]
        XCTAssertEqual(pick(frame, called: "Docs - Browser", rows), .found(1))
        XCTAssertEqual(pick(frame, called: "Inbox - Browser", rows), .found(0))
    }

    func testANameThatDoesNotTellStaysRefused() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        let twins: [Named] = [(nil, frame, "New Tab"), (nil, frame, "New Tab")]
        XCTAssertEqual(pick(frame, called: "New Tab", twins), .ambiguous, "both are called that")
        XCTAssertEqual(pick(frame, called: "Elsewhere", twins), .ambiguous, "neither is called that")
        XCTAssertEqual(pick(frame, called: nil, twins), .ambiguous, "the list gave no name")
        XCTAssertEqual(pick(frame, called: "", twins), .ambiguous, "an empty name names nothing")
        let unnamed: [Named] = [(nil, frame, nil), (nil, frame, nil)]
        XCTAssertEqual(pick(frame, called: "New Tab", unnamed), .ambiguous, "the windows carry no name")
    }

    func testANameIsAskedOnlyWhereTheFrameIsShared() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        XCTAssertEqual(
            pick(frame, called: "Renamed since", [(nil, frame, "Old name")]), .found(0),
            "one window at the frame is that window, whatever it is called now")
        XCTAssertEqual(
            pick(frame, called: "Docs", [(5, frame, "Docs"), (nil, frame, "Other")]), .found(1),
            "a window that carries another number is never it, whatever it is called")
        XCTAssertEqual(pick(frame, called: "Docs", [(77, CGRect(x: 0, y: 0, width: 1, height: 1), "Other")]), .found(0))
    }

    func testTheRefusalSaysWhatStandsThereAndWhatToDo() {
        let said = DesktopWindowPick.ambiguousSaid(77)
        XCTAssertTrue(said.contains("window 77"))
        XCTAssertTrue(said.contains("another window of the same app"))
        XCTAssertTrue(said.contains("move or resize"))
        XCTAssertFalse(said.contains("Accessibility"), "it is not a permission that is missing")
    }

    private typealias Covering = (frame: CGRect?, title: String?, focused: Bool)

    private func pickCovering(_ bounds: CGRect, called title: String?, _ windows: [Covering]) -> DesktopWindowPick.Pick {
        DesktopWindowPick.pickCovering(targetBounds: bounds, targetTitle: title, windows: windows)
    }

    /// A target that is no window of its own in the app's list — a popover —
    /// is held by the window that covers where it stands.
    func testATargetNoWindowStandsAtIsHeldByTheOneThatCoversIt() {
        let popover = CGRect(x: 300, y: 200, width: 200, height: 120)
        let main = CGRect(x: 100, y: 100, width: 900, height: 700)
        let far = CGRect(x: 2_000, y: 100, width: 900, height: 700)
        XCTAssertEqual(pickCovering(popover, called: nil, [(far, "B", false), (main, "A", false)]), .found(1))
        XCTAssertEqual(pickCovering(popover, called: "A", [(far, "A", true)]), .none, "neither a name nor the focus makes a window cover it")
        XCTAssertEqual(pickCovering(popover, called: nil, [(nil, "A", true)]), .none, "a frame that cannot be read covers nothing")
        XCTAssertEqual(pickCovering(popover, called: nil, []), .none)
    }

    /// Several that cover it (2026-11-11): the first of them was taken, which
    /// could read and press a window the look did not picture.
    func testSeveralThatCoverAreToldByNameThenByTheFocusNeverByWhichComesFirst() {
        let popover = CGRect(x: 300, y: 200, width: 200, height: 120)
        let main = CGRect(x: 100, y: 100, width: 900, height: 700)
        let twin = main.offsetBy(dx: 20, dy: 20)
        XCTAssertEqual(
            pickCovering(popover, called: "Docs", [(main, "Inbox", true), (twin, "Docs", false)]), .found(1),
            "the one called what the target is called, before the focus")
        XCTAssertEqual(
            pickCovering(popover, called: nil, [(main, "Inbox", false), (twin, "Docs", true)]), .found(1),
            "the one the app is focused in")
        XCTAssertEqual(
            pickCovering(popover, called: "Elsewhere", [(main, "Inbox", true), (twin, "Docs", false)]), .found(0),
            "a name that tells nothing leaves it to the focus")
        XCTAssertEqual(
            pickCovering(popover, called: "Docs", [(main, "Docs", false), (twin, "Docs", true)]), .found(1),
            "a name both carry leaves it to the focus")
        XCTAssertEqual(
            pickCovering(popover, called: nil, [(main, "Inbox", false), (twin, "Docs", false)]), .ambiguous,
            "nothing tells: not the first")
        XCTAssertEqual(
            pickCovering(popover, called: "", [(main, "Inbox", false), (twin, "", false)]), .ambiguous,
            "an empty name names nothing")
    }

    func testCoveringIsThreeQuartersOfTheSmallerFrame() {
        let target = CGRect(x: 0, y: 0, width: 100, height: 100)
        XCTAssertEqual(DesktopWindowPick.coverShare, 0.75)
        XCTAssertTrue(DesktopWindowPick.covers(CGRect(x: 25, y: 0, width: 100, height: 100), target), "75 of 100 shared")
        XCTAssertFalse(DesktopWindowPick.covers(CGRect(x: 26, y: 0, width: 100, height: 100), target), "74 of 100 shared")
        XCTAssertTrue(DesktopWindowPick.covers(CGRect(x: -500, y: -500, width: 2_000, height: 2_000), target), "the target lies inside it")
        XCTAssertTrue(DesktopWindowPick.covers(CGRect(x: 40, y: 40, width: 20, height: 20), target), "it lies inside the target")
        XCTAssertFalse(DesktopWindowPick.covers(CGRect(x: 500, y: 500, width: 100, height: 100), target), "they do not touch")
    }

    func testTheRefusalOfSeveralThatCoverSaysSoAndWhatToDo() {
        let said = DesktopWindowPick.coveredSaid(77)
        XCTAssertTrue(said.contains("window 77"))
        XCTAssertTrue(said.contains("more than one of them covers"))
        XCTAssertTrue(said.contains("without a window selector"))
        XCTAssertFalse(said.contains("granted"), "it is not a permission that is missing")
    }

    func testNoNumberAndNoFrameThatFitsIsNone() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        XCTAssertEqual(pick(frame, [(nil, CGRect(x: 0, y: 0, width: 400, height: 300))]), .none)
        XCTAssertEqual(pick(frame, []), .none)
        XCTAssertEqual(pick(nil, [(nil, frame)]), .none, "the window list's bounds were not read")
    }

    func testAWindowThatCarriesAnotherNumberIsNeverTheFrameMatch() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        XCTAssertEqual(pick(frame, [(5, frame)]), .none)
        XCTAssertEqual(pick(frame, [(5, frame), (nil, frame)]), .found(1))
    }

    func testAWindowWhoseFrameCannotBeReadIsSkipped() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        XCTAssertEqual(pick(frame, [(nil, nil), (nil, frame)]), .found(1))
    }

    func testTheSameToleranceIsWhatTheRecoveryPathUses() {
        XCTAssertTrue(DesktopWindowPick.framesMatch(CGRect(x: 0, y: 0, width: 10, height: 10), CGRect(x: 2, y: 2, width: 12, height: 8)))
        XCTAssertFalse(DesktopWindowPick.framesMatch(CGRect(x: 0, y: 0, width: 10, height: 10), CGRect(x: 2.1, y: 0, width: 10, height: 10)))
    }
}
