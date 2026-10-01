import CoreGraphics
import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// The pointer's own picture on the window list (t-12979), and whose window
/// a press at a point lands on with it there. Nothing here reads a screen:
/// every window list is written out.
final class PointerPictureTests: XCTestCase {
    /// The level the rows were read at on 09-30; the platform's own level is
    /// checked equal to it below.
    private let cursorLevel = 2_147_483_630

    private func info(pid: pid_t, owner: String, layer: Int, _ bounds: CGRect) -> [String: Any] {
        [
            kCGWindowLayer as String: layer, kCGWindowAlpha as String: CGFloat(1),
            kCGWindowBounds as String: bounds.dictionaryRepresentation as NSDictionary,
            kCGWindowOwnerPID as String: pid, kCGWindowOwnerName as String: owner,
        ]
    }

    /// The one shared table of rows read on this Mac and of guards: answered
    /// here as the window's `is_pointer_picture` answers it.
    func testEveryExampleOfTheSharedTableIsAnsweredAsTheTableSays() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .appendingPathComponent("../../../../../zerocode-core/fixtures/pointer-picture/examples.json").standardized
        let table = try JSONSerialization.jsonObject(with: Data(contentsOf: root)) as? [String: Any]
        let examples = try XCTUnwrap(table?["examples"] as? [[String: Any]])
        XCTAssertGreaterThanOrEqual(examples.count, 7)
        XCTAssertEqual(Int(CGWindowLevelForKey(.cursorWindow)), cursorLevel, "the platform's cursor level is the one read")
        for example in examples {
            let bounds = CGRect(x: 0, y: 0, width: try XCTUnwrap(example["width"] as? Double), height: try XCTUnwrap(example["height"] as? Double))
            XCTAssertEqual(
                DesktopPointerPicture.isPointerPicture(
                    layer: try XCTUnwrap(example["layer"] as? Int), ownerName: try XCTUnwrap(example["owner"] as? String),
                    bounds: bounds, cursorLevel: cursorLevel),
                try XCTUnwrap(example["pointer"] as? Bool), "\(example["what"] ?? "")")
        }
    }

    /// The safety hole of 09-30: the pointer resting over ZeroCode's own
    /// window is the frontmost row at the point, and was taken for the
    /// window a click lands on. Passed over, the click lands on ZeroCode's.
    func testAPressWhereThePointerRestsLandsOnTheWindowUnderIt() {
        let infos = [
            info(pid: 399, owner: "Window Server", layer: cursorLevel, CGRect(x: 590, y: 390, width: 23, height: 22)),
            info(pid: 500, owner: "ZeroCode", layer: 0, CGRect(x: 0, y: 0, width: 1200, height: 800)),
        ]
        let owner = DesktopFrontOwner.pid(at: CGPoint(x: 600, y: 400), infos: infos, displays: [], cursorLevel: cursorLevel) { _ in nil }
        XCTAssertEqual(owner, 500)
    }

    /// A window at the cursor's level that is not all three of the pointer
    /// is the window a press lands on, as before.
    func testAWindowShortOfThePointerIsStillTheOneAPressLandsOn() {
        let under = info(pid: 500, owner: "ZeroCode", layer: 0, CGRect(x: 0, y: 0, width: 1200, height: 800))
        for over in [
            info(pid: 50, owner: "Other", layer: cursorLevel, CGRect(x: 590, y: 390, width: 23, height: 22)),
            info(pid: 399, owner: "Window Server", layer: cursorLevel, CGRect(x: 300, y: 200, width: 500, height: 400)),
        ] {
            let owner = DesktopFrontOwner.pid(at: CGPoint(x: 600, y: 400), infos: [over, under], displays: [], cursorLevel: cursorLevel) { _ in nil }
            XCTAssertEqual(owner, over[kCGWindowOwnerPID as String] as? pid_t)
        }
    }
}
