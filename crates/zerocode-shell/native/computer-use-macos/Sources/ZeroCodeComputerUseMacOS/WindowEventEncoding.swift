import AppKit
import CoreGraphics
import ZeroCodeComputerUseMacOSCore

enum WindowEventEncoding {
    static func privateSource() -> CGEventSource? {
        guard let source = CGEventSource(stateID: .privateState) else { return nil }
        source.localEventsSuppressionInterval = 0
        let permitted: CGEventFilterMask = [.permitLocalMouseEvents, .permitLocalKeyboardEvents, .permitSystemDefinedEvents]
        source.setLocalEventsFilterDuringSuppressionState(permitted, state: .eventSuppressionStateSuppressionInterval)
        source.setLocalEventsFilterDuringSuppressionState(permitted, state: .eventSuppressionStateRemoteMouseDrag)
        return source
    }

    static func make(_ event: HandEvent, mainDisplayHeight: Double, source suppliedSource: CGEventSource? = nil) -> CGEvent? {
        guard case let .window(pid, windowID) = event.route,
              pid > 0, windowID > 0,
              event.x.isFinite, event.y.isFinite,
              mainDisplayHeight.isFinite, mainDisplayHeight > 0,
              let source = suppliedSource ?? privateSource(),
              source.sourceStateID != .combinedSessionState,
              source.sourceStateID != .hidSystemState
        else { return nil }

        let location = CGPoint(x: event.x, y: mainDisplayHeight - event.y)
        let flags = NSEvent.ModifierFlags(rawValue: UInt(event.flags))
        let encoded: CGEvent?
        switch event.kind {
        case .pointerMove:
            encoded = mouse(.mouseMoved, location: location, flags: flags, window: windowID, button: .left, clicks: 0, pressure: 0)
        case let .buttonDown(button, clicks):
            let type: NSEvent.EventType = button == .left ? .leftMouseDown : button == .right ? .rightMouseDown : .otherMouseDown
            encoded = mouse(type, location: location, flags: flags, window: windowID, button: button, clicks: clicks, pressure: 1)
        case let .buttonUp(button, clicks):
            let type: NSEvent.EventType = button == .left ? .leftMouseUp : button == .right ? .rightMouseUp : .otherMouseUp
            encoded = mouse(type, location: location, flags: flags, window: windowID, button: button, clicks: clicks, pressure: 0)
        case let .buttonDrag(button, clicks):
            let type: NSEvent.EventType = button == .left ? .leftMouseDragged : button == .right ? .rightMouseDragged : .otherMouseDragged
            encoded = mouse(type, location: location, flags: flags, window: windowID, button: button, clicks: clicks, pressure: 1)
        case let .key(code, down, modifier):
            encoded = key(code: code, down: down, modifier: modifier != 0, flags: flags, window: windowID, location: location, source: source)
        case let .text(unit, down):
            guard let scalar = Unicode.Scalar(UInt32(unit)) else { return nil }
            encoded = unicode(String(scalar), down: down, flags: flags, window: windowID, location: location)
        case let .unicode(text, down):
            encoded = unicode(text, down: down, flags: flags, window: windowID, location: location)
        case let .scroll(wheel1, wheel2):
            encoded = scroll(dx: wheel2, dy: wheel1, location: location, flags: flags, window: windowID)
        }
        guard let encoded,
              NSEvent(cgEvent: encoded)?.windowNumber == Int(windowID)
        else { return nil }
        encoded.setSource(source)
        encoded.setIntegerValueField(.eventTargetUnixProcessID, value: Int64(pid))
        if event.points {
            encoded.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(windowID))
            encoded.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(windowID))
        }
        if event.points,
           abs(encoded.location.x - event.x) > 0.001 || abs(encoded.location.y - event.y) > 0.001 {
            return nil
        }
        return encoded
    }

    private static func mouse(
        _ type: NSEvent.EventType, location: CGPoint, flags: NSEvent.ModifierFlags,
        window: UInt32, button: MouseButtonSelection, clicks: Int64, pressure: Float
    ) -> CGEvent? {
        guard let event = NSEvent.mouseEvent(
            with: type, location: location, modifierFlags: flags,
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: Int(window),
            context: nil, eventNumber: 1, clickCount: Int(clicks), pressure: pressure
        )?.cgEvent else { return nil }
        let number: Int64 = button == .left ? 0 : button == .right ? 1 : 2
        event.setIntegerValueField(.mouseEventButtonNumber, value: number)
        return event
    }

    private static func key(
        code: UInt16, down: Bool, modifier: Bool, flags: NSEvent.ModifierFlags, window: UInt32, location: CGPoint, source: CGEventSource
    ) -> CGEvent? {
        guard let physical = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else { return nil }
        let changesFlags = modifier || physical.type == .flagsChanged
        var characters = ""
        var unmodified = ""
        if !changesFlags {
            guard physical.type == .keyDown || physical.type == .keyUp else { return nil }
            physical.flags = CGEventFlags(rawValue: UInt64(flags.rawValue))
            guard let translated = NSEvent(cgEvent: physical) else { return nil }
            characters = translated.characters ?? ""
            unmodified = translated.charactersIgnoringModifiers ?? ""
        }
        return NSEvent.keyEvent(
            with: changesFlags ? .flagsChanged : down ? .keyDown : .keyUp, location: location, modifierFlags: flags,
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: Int(window),
            context: nil, characters: characters, charactersIgnoringModifiers: unmodified, isARepeat: false, keyCode: code
        )?.cgEvent
    }

    private static func scroll(
        dx: Int32, dy: Int32, location: CGPoint, flags: NSEvent.ModifierFlags, window: UInt32
    ) -> CGEvent? {
        guard let event = mouse(.mouseMoved, location: location, flags: flags, window: window, button: .left, clicks: 0, pressure: 0),
              let wheel = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: dy, wheel2: dx, wheel3: 0)
        else { return nil }
        event.type = .scrollWheel
        for field: CGEventField in [.scrollWheelEventIsContinuous, .scrollWheelEventDeltaAxis1, .scrollWheelEventDeltaAxis2,
                                   .scrollWheelEventPointDeltaAxis1, .scrollWheelEventPointDeltaAxis2] {
            event.setIntegerValueField(field, value: wheel.getIntegerValueField(field))
        }
        for field: CGEventField in [.scrollWheelEventFixedPtDeltaAxis1, .scrollWheelEventFixedPtDeltaAxis2] {
            event.setDoubleValueField(field, value: wheel.getDoubleValueField(field))
        }
        return event
    }

    private static func unicode(
        _ text: String, down: Bool, flags: NSEvent.ModifierFlags, window: UInt32, location: CGPoint
    ) -> CGEvent? {
        guard !text.isEmpty, text.utf16.count <= WindowTextChunks.maxUTF16Count,
              let event = NSEvent.keyEvent(
                  with: down ? .keyDown : .keyUp, location: location, modifierFlags: flags,
                  timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: Int(window),
                  context: nil, characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: 0
              )?.cgEvent
        else { return nil }
        let units = Array(text.utf16)
        units.withUnsafeBufferPointer { buffer in
            event.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: buffer.baseAddress)
        }
        return event
    }
}

final class LockedWindowEventPoster: @unchecked Sendable {
    private let lock = NSLock()
    private var source: CGEventSource?

    func post(_ event: HandEvent, tag: Int64) throws {
        lock.lock()
        defer { lock.unlock() }
        if source == nil { source = WindowEventEncoding.privateSource() }
        guard case let .window(pid, _) = event.route,
              let source,
              let encoded = WindowEventEncoding.make(event, mainDisplayHeight: CGDisplayBounds(CGMainDisplayID()).height, source: source)
        else {
            throw ProviderError.coded("background_input_unavailable", "window-scoped input could not be encoded; no desktop fallback was attempted")
        }
        encoded.setIntegerValueField(.eventSourceUserData, value: tag)
        encoded.postToPid(pid)
    }
}
