@testable import ZeroCodeComputerUseMacOSCore
import XCTest

final class SettingsWindowIdentityTests: XCTestCase {
    func testIdentifierAnswersRegardlessOfTheLocalizedName() {
        // The live report: a Korean machine owns those windows as "시스템 설정",
        // and the assistant that compared display names never found one.
        XCTAssertTrue(
            SettingsWindowIdentity.isSettings(
                bundleIdentifier: "com.apple.systempreferences",
                ownerName: "시스템 설정"
            )
        )
        XCTAssertTrue(
            SettingsWindowIdentity.isSettings(
                bundleIdentifier: "com.apple.SystemSettings",
                ownerName: "システム設定"
            )
        )
    }

    func testAKnownIdentifierThatDoesNotMatchSettlesIt() {
        // A window is not System Settings because something else wears the name.
        XCTAssertFalse(
            SettingsWindowIdentity.isSettings(
                bundleIdentifier: "com.example.impostor",
                ownerName: "System Settings"
            )
        )
    }

    func testTheEnglishNameStandsInWhenTheOwnerIsAlreadyGone() {
        XCTAssertTrue(
            SettingsWindowIdentity.isSettings(bundleIdentifier: nil, ownerName: "System Settings")
        )
        XCTAssertTrue(
            SettingsWindowIdentity.isSettings(bundleIdentifier: nil, ownerName: "System Preferences")
        )
        XCTAssertFalse(
            SettingsWindowIdentity.isSettings(bundleIdentifier: nil, ownerName: "시스템 설정")
        )
        XCTAssertFalse(SettingsWindowIdentity.isSettings(bundleIdentifier: nil, ownerName: nil))
    }
}
