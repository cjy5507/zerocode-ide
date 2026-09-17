import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class PermissionSettingsTargetTests: XCTestCase {
    func testSharedTableOpensTheExactPrivacyPane() throws {
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let data = try Data(contentsOf: package.appendingPathComponent("permissions.json"))
        for (id, pane) in [("accessibility", "Privacy_Accessibility"), ("screenshots", "Privacy_ScreenCapture")] {
            XCTAssertEqual(PermissionSettingsTarget.settingsURL(id: id, data: data), "x-apple.systempreferences:com.apple.preference.security?\(pane)")
        }
        XCTAssertNil(PermissionSettingsTarget.settingsURL(id: "camera", data: data))
        XCTAssertNil(PermissionSettingsTarget.settingsURL(id: "screenshots", data: Data()))
    }
}
