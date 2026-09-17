import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class PermissionSubjectTests: XCTestCase {
    let shipped = URL(fileURLWithPath: "/Applications/ZeroCode.app/Contents/Resources/ZeroCode Computer Use.app")
    let dev = URL(fileURLWithPath: "/repo/native/computer-use-macos/.build/release/ZeroCode Computer Use.app")

    func testScreenRecordingDragsTheEnclosingApp() {
        XCTAssertEqual(PermissionSubjectResolver.judgedBundleURL(subject: .app, helperURL: shipped).path, "/Applications/ZeroCode.app")
    }

    func testAccessibilityDragsTheHelperItself() {
        XCTAssertEqual(PermissionSubjectResolver.judgedBundleURL(subject: .helper, helperURL: shipped).path, shipped.path)
    }

    func testAHelperStandingAloneIsItsOwnResponsibleProcess() {
        XCTAssertEqual(PermissionSubjectResolver.judgedBundleURL(subject: .app, helperURL: dev).path, dev.path)
    }
}
