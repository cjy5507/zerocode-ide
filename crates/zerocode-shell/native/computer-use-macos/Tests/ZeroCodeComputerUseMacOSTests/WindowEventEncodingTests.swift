import AppKit
import XCTest
@testable import ZeroCodeComputerUseMacOSCore
@testable import ZeroCodeComputerUseMacOS

final class WindowEventEncodingTests: XCTestCase {
    func testLiteralPasteDoesNotTurnControlCharactersIntoKeyboardActions() {
        for text in ["first\tsecond", "line\nnext", "\u{1B}", "\u{9B}"] {
            XCTAssertTrue(KeyboardInputSafety.containsControlCharacters(text))
        }
        XCTAssertFalse(KeyboardInputSafety.containsControlCharacters("한글 👨‍👩‍👧‍👦 café"))
    }

    func testEveryChordReleaseKeepsItsWindowEvenWithoutAHeldModifierFlag() throws {
        let route = HandRoute.window(pid: 42, id: 123)
        let source = try XCTUnwrap(WindowEventEncoding.privateSource())
        for key in ["cmd+a", "shift+tab", "cmd+alt+shift+f", "escape", "left"] {
            let stroke = try KeyMap.parse(key).stroke
            for input in stroke.downs(route: route) + stroke.ups(route: route) {
                let encoded = try XCTUnwrap(WindowEventEncoding.make(input, mainDisplayHeight: CGDisplayBounds(CGMainDisplayID()).height, source: source), "\(key) \(input)")
                XCTAssertEqual(NSEvent(cgEvent: encoded)?.windowNumber, 123)
                let data = try XCTUnwrap(encoded.data)
                let restored = try XCTUnwrap(CGEvent(withDataAllocator: nil, data: data))
                XCTAssertEqual(NSEvent(cgEvent: restored)?.windowNumber, 123)
            }
        }
    }
    func testShortcutsCarryTheKeyboardLayoutsCharacters() throws {
        let source = try XCTUnwrap(WindowEventEncoding.privateSource())
        let physical = try XCTUnwrap(CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true))
        physical.flags = .maskCommand
        let expected = try XCTUnwrap(NSEvent(cgEvent: physical))
        let encoded = try XCTUnwrap(WindowEventEncoding.make(
            HandEvent(.key(code: 0, down: true, modifier: 0), flags: CGEventFlags.maskCommand.rawValue, route: .window(pid: 42, id: 123)),
            mainDisplayHeight: CGDisplayBounds(CGMainDisplayID()).height, source: source
        ))
        let native = try XCTUnwrap(NSEvent(cgEvent: encoded))
        XCTAssertEqual(native.characters, expected.characters)
        XCTAssertEqual(native.charactersIgnoringModifiers, expected.charactersIgnoringModifiers)
        XCTAssertEqual(native.windowNumber, 123)
    }
    func testModifiersUseFlagsChangedWithoutBecomingCharacterKeys() throws {
        let event = try XCTUnwrap(WindowEventEncoding.make(
            HandEvent(.key(code: 55, down: true, modifier: CGEventFlags.maskCommand.rawValue), flags: CGEventFlags.maskCommand.rawValue, route: .window(pid: 42, id: 123)),
            mainDisplayHeight: 1000
        ))
        XCTAssertEqual(event.type, .flagsChanged)
        XCTAssertEqual(NSEvent(cgEvent: event)?.windowNumber, 123)
        XCTAssertEqual(event.getIntegerValueField(.keyboardEventKeycode), 55)
    }

    func testPixelScrollRetainsItsWindowAndLocalPosition() throws {
        let event = try XCTUnwrap(WindowEventEncoding.make(
            HandEvent(.scroll(wheel1: -90, wheel2: 12), x: 100, y: 150, route: .window(pid: 42, id: 123)),
            mainDisplayHeight: CGDisplayBounds(CGMainDisplayID()).height
        ))
        let native = try XCTUnwrap(NSEvent(cgEvent: event))
        XCTAssertEqual(native.type, .scrollWheel)
        XCTAssertEqual(native.windowNumber, 123)
        XCTAssertEqual(event.location, CGPoint(x: 100, y: 150))
        XCTAssertEqual(native.scrollingDeltaY, -90)
        XCTAssertEqual(native.scrollingDeltaX, 12)
        XCTAssertTrue(native.hasPreciseScrollingDeltas)
    }
    func testWindowNumberSurvivesEventSerialization() throws {
        let input = HandEvent(.buttonDown(.left, clickState: 1), x: 31.5, y: 42.25, route: .window(pid: 42, id: 73))
        let event = try XCTUnwrap(WindowEventEncoding.make(input, mainDisplayHeight: CGDisplayBounds(CGMainDisplayID()).height))
        let data = try XCTUnwrap(event.data)
        let copy = try XCTUnwrap(CGEvent(withDataAllocator: nil, data: data))
        XCTAssertEqual(NSEvent(cgEvent: copy)?.windowNumber, 73)
        XCTAssertEqual(copy.location, CGPoint(x: 31.5, y: 42.25))
    }

    func testWindowInputHasAPrivateSourceEvenIfTheCallerNamesSessionState() throws {
        let input = HandEvent(.key(code: 0, down: true, modifier: 0), route: .window(pid: 42, id: 73), source: .session)
        let source = try XCTUnwrap(WindowEventEncoding.privateSource())
        let event = try XCTUnwrap(WindowEventEncoding.make(input, mainDisplayHeight: CGDisplayBounds(CGMainDisplayID()).height, source: source))
        XCTAssertEqual(event.getIntegerValueField(.eventSourceStateID), Int64(source.sourceStateID.rawValue))
        XCTAssertNotEqual(source.sourceStateID, .combinedSessionState)
        XCTAssertNotEqual(source.sourceStateID, .hidSystemState)
        XCTAssertEqual(source.localEventsSuppressionInterval, 0)
    }

    func testUnicodeIsEncodedRatherThanRetranslatedFromThePhysicalKey() throws {
        let input = HandEvent(.unicode("bg한글🙂", down: true), route: .window(pid: 42, id: 73))
        let event = try XCTUnwrap(WindowEventEncoding.make(input, mainDisplayHeight: CGDisplayBounds(CGMainDisplayID()).height))
        var units = [UniChar](repeating: 0, count: WindowTextChunks.maxUTF16Count)
        var count = 0
        event.keyboardGetUnicodeString(maxStringLength: units.count, actualStringLength: &count, unicodeString: &units)
        XCTAssertEqual(String(decoding: units.prefix(count), as: UTF16.self), "bg한글🙂")
    }

    func testAWindowEncoderCannotManufactureADesktopEvent() {
        let input = HandEvent(.buttonDown(.left, clickState: 1), x: 31, y: 42)
        XCTAssertNil(WindowEventEncoding.make(input, mainDisplayHeight: 1080))
    }

    func testSharedEventSourcesCannotEnterTheWindowTransport() throws {
        let source = try XCTUnwrap(CGEventSource(stateID: .combinedSessionState))
        let input = HandEvent(.key(code: 0, down: true, modifier: 0), route: .window(pid: 42, id: 73))
        XCTAssertNil(WindowEventEncoding.make(input, mainDisplayHeight: CGDisplayBounds(CGMainDisplayID()).height, source: source))
    }

    func testInvalidRecipientsAndNonFinitePointsAreRefused() {
        for input in [
            HandEvent(.pointerMove, route: .window(pid: 0, id: 73)),
            HandEvent(.pointerMove, route: .window(pid: 42, id: 0)),
            HandEvent(.pointerMove, x: .nan, route: .window(pid: 42, id: 73)),
            HandEvent(.pointerMove, y: .infinity, route: .window(pid: 42, id: 73)),
        ] {
            XCTAssertNil(WindowEventEncoding.make(input, mainDisplayHeight: 1080))
        }
    }

    func testUnicodeChunksRemainBoundedWithoutSplittingSurrogatePairs() {
        let text = String(repeating: "한🙂a", count: 30)
        let chunks = Array(WindowTextChunks(text))
        XCTAssertEqual(chunks.joined(), text)
        XCTAssertTrue(chunks.allSatisfy { $0.utf16.count <= WindowTextChunks.maxUTF16Count })
    }

    func testEmptyTextProducesNoChunks() {
        XCTAssertTrue(Array(WindowTextChunks("")).isEmpty)
    }

    func testFocusMovingControlsAreSeparateUnicodeEvents() {
        XCTAssertEqual(Array(WindowTextChunks("first\tsecond\nthird")), ["first", "\t", "second", "\n", "third"])
    }

    func testBackgroundShortcutPolicyRefusesSharedClipboardAccess() {
        for chord in ["cmd+c", "command+shift+x", "meta+v", "shift+insert", "control+insert", "shift+delete"] {
            XCTAssertTrue(KeyboardInputSafety.usesClipboard(chord), chord)
        }
        for chord in ["cmd+a", "cmd+f", "ctrl+c", "tab", "left"] {
            XCTAssertFalse(KeyboardInputSafety.usesClipboard(chord), chord)
        }
    }
}
