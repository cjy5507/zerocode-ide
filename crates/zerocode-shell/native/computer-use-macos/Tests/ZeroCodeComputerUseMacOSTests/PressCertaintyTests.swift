import CoreFoundation
import XCTest
@testable import ZeroCodeComputerUseMacOS
@testable import ZeroCodeComputerUseMacOSCore

final class PressCertaintyTests: XCTestCase {
    private struct AppWithoutLaunchDate: PressApplicationFacts {
        var processIdentifier: Int32 = 10
        var isTerminated = false
        let launchDate: Date? = nil
        var bundleURL: URL? = URL(fileURLWithPath: "/Applications/Example.app")
        var executableURL: URL? = URL(fileURLWithPath: "/Applications/Example.app/Contents/MacOS/Example")
    }
    private let shortcut = MenuShortcut(key: "t", command: true, shift: false, option: false, control: false)!

    private func source(_ file: String = "main.swift") throws -> String {
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        return try String(contentsOf: package.appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/\(file)"), encoding: .utf8)
    }

    private func target(item: Int, pid: Int32 = 10, start: UInt64 = 20, window: Int = 20, defaultButton: Int? = nil) -> PressCertainty.Target<Int> {
        let application = PressCertainty.ApplicationInstance(pid: pid, started: start, location: URL(fileURLWithPath: "/Applications/Example.app"))
        return PressCertainty.Target(application: application, window: window, item: item,
                                     defaultButton: defaultButton, label: "Empty Trash")
    }

    func testApprovalMatchesTheSameAppWithoutALaunchDate() throws {
        let app = AppWithoutLaunchDate()
        XCTAssertNil(app.launchDate)

        func readTarget(_ facts: AppWithoutLaunchDate, start: UInt64?) -> PressCertainty.Target<Int>? {
            let application = pressApplicationInstance(facts, started: { _ in start })
            return application.map {
                PressCertainty.Target(application: $0, window: 20, item: 30, label: "Empty Trash")
            }
        }
        let approved = try XCTUnwrap(readTarget(app, start: 20))
        try PressCertainty.requireTarget(approved, actual: readTarget(app, start: 20), same: ==)

        var relaunched = app
        relaunched.processIdentifier += 1
        var replaced = app
        replaced.bundleURL = URL(fileURLWithPath: "/Applications/Replacement.app")
        var terminated = app
        terminated.isTerminated = true
        let changed = [
            readTarget(app, start: 21), readTarget(relaunched, start: 20),
            readTarget(replaced, start: 20), readTarget(app, start: nil), readTarget(terminated, start: 20),
        ]
        for actual in changed {
            XCTAssertThrowsError(try PressCertainty.requireTarget(approved, actual: actual, same: ==)) {
                XCTAssertEqual($0 as? PressCertainty.Refusal, .targetChanged)
            }
        }
    }

    func testAnUnbundledAppCanBeBoundToItsExecutable() throws {
        var app = AppWithoutLaunchDate()
        app.bundleURL = nil
        XCTAssertEqual(pressApplicationInstance(app, started: { _ in 20 })?.location, app.executableURL)
        app.executableURL = nil
        XCTAssertNil(pressApplicationInstance(app, started: { _ in 20 }))
    }

    func testProcessStartReadsThePublicKernelRecord() throws {
        let code = try source("ReflexRuntime.swift")
        XCTAssertFalse(code.contains("proc_pidinfo("))
        XCTAssertTrue(code.contains("KERN_PROC_PID"))
        XCTAssertEqual(try XCTUnwrap(ProcessStart.of(getpid())), ProcessStart.of(getpid()))
        XCTAssertNil(ProcessStart.of(Int32.max))
    }

    func testApprovalBindsAXIdentityEvenForSameTitle() throws {
        let allowed = target(item: 30)
        var presses = 0
        for changed in [target(item: 31), target(item: 30, pid: 11),
                        target(item: 30, start: 21), target(item: 30, window: 21)] {
            XCTAssertThrowsError(try PressCertainty.requireTarget(allowed, actual: changed, same: ==)) {
                XCTAssertEqual($0 as? PressCertainty.Refusal, .targetChanged)
            }
            if allowed.matches(changed, same: ==) { presses += 1 }
        }
        XCTAssertEqual(presses, 0)
        try PressCertainty.requireTarget(allowed, actual: target(item: 30), same: ==)
        XCTAssertThrowsError(try PressCertainty.requireTarget(target(item: 30, defaultButton: 50),
                                                             actual: target(item: 30, defaultButton: 51), same: ==)) {
            XCTAssertEqual($0 as? PressCertainty.Refusal, .targetChanged)
        }
    }

    func testDisappearedApprovedItemCannotSendWindowKey() throws {
        let allowed = target(item: 30)
        let road = MenuShortcut.road(chord: "cmd+t", shortcut: shortcut, behind: { true },
                                     menus: { .read([0, 1, 2]) }, children: { _ in [] }, equivalent: { _ in (.read(nil), .read(nil)) })
        guard case .windowKey = road else { return XCTFail("a complete search found no item") }
        var keys = 0
        do {
            try PressCertainty.requireTarget(allowed, actual: nil, same: ==)
            keys += 1
        } catch {
            XCTAssertEqual(error as? PressCertainty.Refusal, .targetChanged)
        }
        XCTAssertEqual(keys, 0)

        let code = try source()
        let retry = try XCTUnwrap(code.range(of: "try validatePressApproval("))
        let send = try XCTUnwrap(code.range(of: "try hotkey(params: params, shortcut: shortcutForDispatch, snapshot: snapshotForDispatch)"))
        XCTAssertLessThan(retry.lowerBound, send.lowerBound)
        XCTAssertTrue(code.contains("approval.target.matches(RequestContext.current.checkedPress"))
    }

    func testClosedMenuRetainsTheWindowWhereItWasRead() throws {
        let numbers = ElementIdentityBook<Int>(hash: { UInt($0) }, equal: ==)
        let old = numbers.number(for: 30, root: "old", surfaceWindow: 20)
        let new = numbers.number(for: 30, root: "new", surfaceWindow: 21)
        XCTAssertNotEqual(old, new)
        var presses = 0
        do {
            try PressCertainty.requireWindow(readFor: 20, target: 21, destinations: { (21, 21) }, same: ==)
            presses += 1
        } catch {
            XCTAssertEqual(error as? PressCertainty.Refusal, .observedWindowChanged)
        }
        XCTAssertEqual(presses, 0)
        try PressCertainty.requireWindow(readFor: 21, target: 21, destinations: { (21, 21) }, same: ==)
        XCTAssertTrue(try source().contains("readFor: record.observedWindow"))
    }

    func testUnreadAndDirectSubmenuChildrenBlockAllPresses() throws {
        var presses = 0
        let reads: [[String]?] = [nil, ["AXMenuItem"], ["AXMenu"]]
        for (children, reason) in zip(reads, [PressCertainty.Refusal.childrenUnread, .opensMenu, .opensMenu]) {
            do {
                try PressCertainty.requireClosedCommand(role: "AXMenuItem", action: "AXPress", children: { children })
                presses += 1
            } catch {
                XCTAssertEqual(error as? PressCertainty.Refusal, reason)
            }
        }
        XCTAssertEqual(presses, 0)
        XCTAssertTrue(try source().contains("children: { childrenAnswered(element) }"))
    }

    func testUnfinishedSearchWithoutAnyMatchesCannotSendWindowKey() {
        for bar in [PressCertainty.Reading<[Int]?>.read([0, 1, 2]), .unread] {
            let road = MenuShortcut.road(chord: "cmd+t", shortcut: shortcut, behind: { true },
                                         menus: { bar }, children: { _ in nil }, equivalent: { _ in (.read(nil), .read(nil)) })
            guard case .unfinished(nil) = road else { return XCTFail("unread menu allowed input") }
        }
    }

    func testUnreadShortcutAttributesRefuseBeforeAnyInput() throws {
        let equivalents: [MenuShortcut.Equivalent] = [
            (.unread, .read(nil)), (.read("T"), .unread), (.read("X"), .unread), (.read("T"), .read(nil)),
        ]
        for equivalent in equivalents {
            let road = MenuShortcut.road(chord: "cmd+t", shortcut: shortcut, behind: { true },
                                         menus: { .read([0, 1, 2]) }, children: { $0 == 2 ? [3] : [] },
                                         equivalent: { _ in equivalent })
            guard case .unfinished(nil) = road else { return XCTFail("unread shortcut attribute allowed input") }
        }
        let code = try source()
        XCTAssertTrue(code.contains("item, kAXMenuItemCmdCharAttribute"))
        XCTAssertTrue(code.contains("item, kAXMenuItemCmdModifiersAttribute"))
    }

    func testDisabledMenuIsReportedBeforeConfirmingItsClickAction() throws {
        let code = try source()
        let label = try XCTUnwrap(code.range(of: "func label(_ record: ElementRecord?"))
        let body = code[label.upperBound...].prefix(1500)
        let disabled = try XCTUnwrap(body.range(of: "try requireMenuEnabled("))
        let actionable = try XCTUnwrap(body.range(of: "ConfirmationTarget.backgroundLabel("))
        XCTAssertLessThan(disabled.lowerBound, actionable.lowerBound)
        XCTAssertTrue(code.contains("MenuBarCommand.menuOwnerRoles.contains(record.role) { return }"))
        XCTAssertTrue(code.contains("MenuBarCommand.disabledWords(app: app, title: title)"))

        for cachedWindow in [false, true] {
            for action in [false, true] {
                var sends = 0
                do {
                    try PressCertainty.requireEnabled(.read(false))
                    if action { sends += 1 }
                    XCTFail("disabled menu with cache=\(cachedWindow), action=\(action) was not refused")
                } catch {
                    XCTAssertEqual(error as? PressCertainty.Refusal, .disabled)
                }
                XCTAssertEqual(sends, 0)
            }
        }
        for read in [PressCertainty.Reading<Bool?>.unread, .read(nil)] {
            XCTAssertThrowsError(try PressCertainty.requireEnabled(read)) {
                XCTAssertEqual($0 as? PressCertainty.Refusal, .enabledUnread)
            }
        }
    }

    func testForegroundIsReadAtAXSendTime() throws {
        let old = PressCertainty.Person(foreground: 1, events: [7, 8])
        var presses = 0
        for now in [PressCertainty.Person(foreground: 10, events: [7, 8]),
                    PressCertainty.Person(foreground: 2, events: [7, 8]), nil] {
            do {
                try old.requireUnchanged(now, targetPID: 10)
                presses += 1
            } catch {
                XCTAssertEqual(error as? PressCertainty.Refusal, .personChanged)
            }
        }
        XCTAssertEqual(presses, 0)
        try old.requireUnchanged(old, targetPID: 10)

        let code = try source()
        let checker = try XCTUnwrap(code.range(of: "private func performActionChecked("))
        let body = code[checker.upperBound...].prefix(1050)
        let lastCheck = try XCTUnwrap(body.range(of: "try beforeSend()"))
        let send = try XCTUnwrap(body.range(of: "AXUIElementPerformAction("))
        XCTAssertLessThan(lastCheck.lowerBound, send.lowerBound)
        XCTAssertTrue(code.contains("try person.requireUnchanged(DesktopBorrowHost.pressPerson(), targetPID: app.pid)"))
        XCTAssertTrue(code.contains("&& error.mayBorrowDesktop && !RequestContext.current.sent"))
    }

    func testOnlyInputInTheTargetAppStopsThePress() throws {
        let before = PressCertainty.Person(foreground: 1, events: [7, 8])
        let after = PressCertainty.Person(foreground: 1, events: [7, 9])
        var sends = 0
        func send(to pid: Int32) throws {
            try before.requireUnchanged(after, targetPID: pid)
            sends += 1
        }
        XCTAssertNoThrow(try send(to: 10))
        XCTAssertThrowsError(try send(to: 1)) {
            XCTAssertEqual($0 as? PressCertainty.Refusal, .personChanged)
        }
        XCTAssertEqual(sends, 1)
    }

    func testUnreportedEnabledStateAllowsNonMenuControlsAndRows() throws {
        try PressCertainty.requireEnabled(.read(nil), mustAnswer: false)
        XCTAssertThrowsError(try PressCertainty.requireEnabled(.read(false), mustAnswer: false)) {
            XCTAssertEqual($0 as? PressCertainty.Refusal, .disabled)
        }
        XCTAssertThrowsError(try PressCertainty.requireEnabled(.unread, mustAnswer: false)) {
            XCTAssertEqual($0 as? PressCertainty.Refusal, .enabledUnread)
        }
    }

    func testEnabledAcceptsBooleanAndNumericAnswers() {
        XCTAssertEqual(pressEnabledValue(kCFBooleanTrue), true)
        XCTAssertEqual(pressEnabledValue(kCFBooleanFalse), false)
        XCTAssertEqual(pressEnabledValue(NSNumber(value: 1)), true)
        XCTAssertEqual(pressEnabledValue(NSNumber(value: 0)), false)
        XCTAssertNil(pressEnabledValue(NSNumber(value: 2)))
        XCTAssertNil(pressEnabledValue("1" as CFString))
    }

    func testSheetMayNameItsOwningWindow() throws {
        let parentsOfSheets = [30: 20, 31: 21]
        func check(_ reported: Int?) throws {
            try PressCertainty.requireContainingWindow(reported, target: 20, same: ==, sheetParent: {
                parentsOfSheets[$0]
            })
        }
        XCTAssertNoThrow(try check(20))
        XCTAssertNoThrow(try check(30))
        for wrong in [21, 31, 32, nil] {
            XCTAssertThrowsError(try check(wrong)) {
                XCTAssertEqual($0 as? PressCertainty.Refusal, .observedWindowChanged)
            }
        }
    }

    func testRemoteElementCanPressOnlyInItsObservedWindow() throws {
        var sends = 0
        func send(containing window: Int?, observed: Int? = 20) throws {
            try PressCertainty.requireElementWindow(
                readFor: observed, target: 20, targetPID: 10, elementPID: { 11 },
                onSurface: false, menuWindows: nil, containingWindow: { window }, same: ==, sheetParent: { _ in nil }
            )
            sends += 1
        }
        for window in [21, nil] {
            XCTAssertThrowsError(try send(containing: window)) {
                XCTAssertEqual($0 as? PressCertainty.Refusal, .observedWindowChanged)
            }
        }
        XCTAssertThrowsError(try send(containing: 20, observed: 21)) {
            XCTAssertEqual($0 as? PressCertainty.Refusal, .observedWindowChanged)
        }
        XCTAssertEqual(sends, 0)
        try send(containing: 20)
        XCTAssertEqual(sends, 1)
        XCTAssertTrue(try source().contains("try PressCertainty.requireElementWindow("))
    }

    func testSurfaceAndMenuElementsStillRequireTheAppsPID() throws {
        func check(pid: Int32?, onSurface: Bool) throws {
            try PressCertainty.requireElementWindow(
                readFor: 20, target: 20, targetPID: 10, elementPID: { pid }, onSurface: onSurface,
                menuWindows: onSurface ? nil : { (20, 20) }, containingWindow: {
                    XCTFail("surface and menu targets use the app's PID")
                    return nil
                }, same: ==, sheetParent: { _ in nil }
            )
        }
        for onSurface in [false, true] {
            XCTAssertNoThrow(try check(pid: 10, onSurface: onSurface))
            for pid in [11, nil] as [Int32?] {
                XCTAssertThrowsError(try check(pid: pid, onSurface: onSurface)) {
                    XCTAssertEqual($0 as? PressCertainty.Refusal, .targetChanged)
                }
            }
        }
    }

    func testRowSelectionReadbackFailureCannotBeVerifiedByOtherEffects() throws {
        let read = RowSelection.chosenAsWanted([1], read: nil, same: ==)
        XCTAssertEqual(ActionDelivery.finished(readBackVerified: read == true, effectSeen: true, requiresReadBack: true), .deliveredUnverified)
        XCTAssertEqual(ActionDelivery.finished(readBackVerified: true, effectSeen: false, requiresReadBack: true), .deliveredVerified)
        XCTAssertTrue(try source().contains("requiresReadBack: name == \"AXSelectedRows\" || name == \"AXSelected\""))
    }

    func testMenuSearchHasAnOverallDeadline() throws {
        var now: TimeInterval = 10
        let deadline = MenuShortcut.Deadline(now: { now })
        var reads: [Int] = []
        let road = MenuShortcut.road(chord: "cmd+t", shortcut: shortcut, behind: { true },
                                     menus: { .read([0, 1, 2]) }, children: { owner in
            reads.append(owner)
            if owner == 2 { return [3, 4] }
            now += TimeInterval(AccessibilityTimeouts.messagingSeconds)
            return []
        }, equivalent: { _ in (.read("T"), .read(0)) }, expired: deadline.expired)
        guard case .unfinished(3?) = road else { return XCTFail("slow app allowed input") }
        XCTAssertEqual(reads, [2, 3])
        XCTAssertTrue(try source().contains("expired: deadline.expired"))
    }
}
