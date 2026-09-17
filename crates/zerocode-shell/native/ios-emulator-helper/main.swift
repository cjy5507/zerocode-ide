// ZeroCode iOS Simulator input helper.
//
// The private SimulatorKit HID protocol is derived from serve-sim
// (https://github.com/EvanBacon/serve-sim), Apache-2.0. This smaller helper
// keeps only the input boundary ZeroCode needs and discovers the active Xcode
// frameworks at runtime; no Orca artifact is linked or loaded.

import AppKit
import CoreVideo
import Darwin
import Foundation
import IOSurface
import ObjectiveC
import VideoToolbox

private struct Request: Decodable {
    let id: UInt64
    let kind: String
    let x: Double?
    let y: Double?
    let x1: Double?
    let y1: Double?
    let x2: Double?
    let y2: Double?
    let x3: Double?
    let y3: Double?
    let x4: Double?
    let y4: Double?
    let durationMs: UInt32?
    let text: String?
    let name: String?
    let rotation: UInt32?
    /// A live touch's phase — "begin", "move" or "end". The mouse is streamed
    /// as the finger it is; replaying a finished drag as one long swipe left
    /// the device deaf until the hand was done, and the replay blocked this
    /// process's single request loop — frames included — for its whole
    /// duration.
    let phase: String?
    /// The seed the caller already has a picture of; a frame request
    /// carrying it is answered with nothing when the screen has not moved.
    let seed: UInt32?
    /// The longest edge the pane can actually show. The device's own
    /// framebuffer is 1260x2736 and a pane is a few hundred points wide,
    /// so this is where the bytes are decided — not by a constant.
    let longEdge: Int?
    /// JPEG quality, 0...1. The pane decides it, because the pane is the only
    /// side that knows what the picture is worth: a mirror somebody is reading
    /// text on wants different bytes from a thumbnail behind another tab.
    let quality: Double?
    /// The most pictures a second a `stream` request may push.
    let maxFps: UInt32?
}

/// What the helper uses when a request does not say.
///
/// Fallbacks, not policy. Every one of these is decided by the caller in
/// practice; they exist so a malformed or older request still produces a
/// picture instead of a division by zero.
private enum FrameTuning {
    static let quality = 0.82
    static let maxFps: UInt32 = 60
    /// How often the pusher re-reads the surface generation while nothing is
    /// changing. Asking costs 35µs, so this is what decides how late a change
    /// can be noticed — 2ms is far below anything a hand can feel, and 500
    /// looks a second at 35µs is under two percent of one core.
    static let idlePoll = 0.002
}

private struct Response: Encodable {
    let id: UInt64
    let ok: Bool
    let error: String?
    let data: String?
    /// Only a frame answers these. `seed` comes back even when `data` does not,
    /// so a caller that asked about an unchanged screen learns nothing changed
    /// rather than having to guess.
    var seed: UInt32?
    var width: Int?
    var height: Int?
}

private enum HelperError: LocalizedError {
    case message(String)

    var errorDescription: String? {
        switch self {
        case .message(let message): message
        }
    }
}

private enum Xcode {
    static func developerDirectory() throws -> String {
        if let configured = ProcessInfo.processInfo.environment["DEVELOPER_DIR"]?
            .trimmingCharacters(in: .whitespacesAndNewlines), !configured.isEmpty {
            return configured
        }
        let pipe = Pipe()
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/xcode-select")
        process.arguments = ["-p"]
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        let value = String(
            data: pipe.fileHandleForReading.readDataToEndOfFile(),
            encoding: .utf8
        )?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard process.terminationStatus == 0, !value.isEmpty else {
            throw HelperError.message("활성 Xcode 개발자 디렉터리를 찾지 못했습니다")
        }
        return value
    }
}

private enum SimulatorFrameworks {
    static func load() throws {
        let developer = try Xcode.developerDirectory()
        let candidates = [
            "/Library/Developer/PrivateFrameworks/CoreSimulator.framework/CoreSimulator",
            "\(developer)/Library/PrivateFrameworks/CoreSimulator.framework/CoreSimulator",
            "\(developer)/../SharedFrameworks/SimulatorKit.framework/SimulatorKit",
            "\(developer)/Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit",
        ]
        var simulatorKitLoaded = false
        for path in candidates {
            if dlopen(path, RTLD_NOW | RTLD_GLOBAL) != nil, path.contains("SimulatorKit") {
                simulatorKitLoaded = true
            }
        }
        guard simulatorKitLoaded else {
            throw HelperError.message("활성 Xcode의 SimulatorKit을 불러오지 못했습니다")
        }
    }
}

func simulatorDevice(udid: String) throws -> NSObject {
    guard let contextClass = NSClassFromString("SimServiceContext") as? NSObject.Type else {
        throw HelperError.message("CoreSimulator 서비스를 찾지 못했습니다")
    }
    let developer = try Xcode.developerDirectory() as NSString
    let shared = NSSelectorFromString("sharedServiceContextForDeveloperDir:error:")
    guard
        let context = contextClass.perform(shared, with: developer, with: nil)?
            .takeUnretainedValue() as? NSObject
    else {
        throw HelperError.message("CoreSimulator 서비스에 연결하지 못했습니다")
    }
    let defaultSet = NSSelectorFromString("defaultDeviceSetWithError:")
    guard
        let deviceSet = context.perform(defaultSet, with: nil)?
            .takeUnretainedValue() as? NSObject,
        let devices = deviceSet.value(forKey: "devices") as? [NSObject]
    else {
        throw HelperError.message("시뮬레이터 기기 목록을 읽지 못했습니다")
    }
    guard let device = devices.first(where: {
        ($0.value(forKey: "UDID") as? NSUUID)?.uuidString == udid
    }) else {
        throw HelperError.message("요청한 iOS 시뮬레이터가 없습니다")
    }
    return device
}

/// The simulator's own framebuffer, read where it is already drawn.
///
/// Everything here was measured on this machine rather than inferred, because
/// the shapes involved are private and two of them are not what their names
/// suggest:
///
///   * The device's `io.ioPorts` are ROCK remote proxies. They answer
///     `respondsToSelector:` and forwarded messages, and they are NOT key-value
///     coding compliant — `valueForKey:"portIdentifier"` raises
///     `NSUnknownKeyException`. Everything below goes through selectors.
///   * There are always TWO `com.apple.framebuffer.display` ports and both
///     descriptors respond to `framebufferSurface`. Which one is real is not a
///     matter of order: one answers a `displaySize` of the device's own screen
///     (measured 1260x2736) and the other answers 0x0. The sized one is the
///     screen, on every launch, attached or hidden.
///
/// `framebufferSurface` is nil while the device is not booted, and that nil is
/// the ONLY signal this road gets that its source has gone — a surface already
/// held keeps answering its last pixels forever, so liveness is re-asked for
/// rather than waited for.
///
/// Hiding Simulator.app does NOT take the surface away: measured across a
/// `set visible ... to false`, the same surface object kept answering. That is
/// the whole reason this road exists — the window-reading road below it can
/// only see an application the person can also see.
private final class FrameSource {
    private let device: NSObject
    private var screen: NSObject?
    private var surface: IOSurfaceRef?
    private var pixels: CVPixelBuffer?
    private var session: VTCompressionSession?
    private var sessionShape: (Int, Int, Double) = (0, 0, 0)
    /// One source, two callers: the request loop answering a `frame`, and the
    /// pusher thread. Everything below mutates cached CoreVideo and
    /// VideoToolbox state, so they take turns rather than racing. Recursive
    /// because `capture` calls `forget` on its own failure paths.
    private let turn = NSRecursiveLock()

    init(device: NSObject) {
        self.device = device
    }

    deinit {
        if let session {
            VTCompressionSessionInvalidate(session)
        }
    }

    private static func answer(_ target: NSObject, _ name: String) -> AnyObject? {
        let selector = NSSelectorFromString(name)
        guard target.responds(to: selector) else { return nil }
        return target.perform(selector)?.takeUnretainedValue()
    }

    private static func displaySize(of descriptor: NSObject) -> CGSize {
        typealias SizeFunction = @convention(c) (AnyObject, Selector) -> CGSize
        let selector = NSSelectorFromString("displaySize")
        guard descriptor.responds(to: selector),
              let implementation = class_getMethodImplementation(type(of: descriptor), selector)
        else { return .zero }
        return unsafeBitCast(implementation, to: SizeFunction.self)(descriptor, selector)
    }

    /// The one descriptor that has a screen — chosen by the size it answers,
    /// never by its position in the list.
    private func resolveScreen() throws -> NSObject {
        if let screen { return screen }
        guard let io = device.value(forKey: "io") as? NSObject,
              let ports = io.value(forKey: "ioPorts") as? [NSObject]
        else {
            throw HelperError.message("시뮬레이터 디스플레이 포트를 읽지 못했습니다")
        }
        for port in ports {
            guard (Self.answer(port, "portIdentifier") as? String) == "com.apple.framebuffer.display",
                  let descriptor = Self.answer(port, "descriptor") as? NSObject
            else { continue }
            let size = Self.displaySize(of: descriptor)
            if size.width > 0, size.height > 0 {
                screen = descriptor
                return descriptor
            }
        }
        throw HelperError.message("시뮬레이터 화면을 찾지 못했습니다")
    }

    /// The surface, re-asked for whenever we do not hold a live one. Holding it
    /// is what makes the frame road cheap: re-reading `framebufferSurface` every
    /// frame is a message to another process, while reading the seed of a
    /// surface already held is a shared-memory load (measured 2.1ns).
    private func liveSurface() throws -> IOSurfaceRef {
        if let surface { return surface }
        let screen = try resolveScreen()
        guard let raw = Self.answer(screen, "framebufferSurface") else {
            throw HelperError.message("시뮬레이터 화면이 아직 없습니다 — 기기가 부팅되지 않았습니다")
        }
        let found = raw as! IOSurfaceRef
        var unmanaged: Unmanaged<CVPixelBuffer>?
        let made = CVPixelBufferCreateWithIOSurface(kCFAllocatorDefault, found, nil, &unmanaged)
        guard made == kCVReturnSuccess, let buffer = unmanaged?.takeRetainedValue() else {
            throw HelperError.message("시뮬레이터 화면을 픽셀 버퍼로 열지 못했습니다: \(made)")
        }
        surface = found
        pixels = buffer
        return found
    }

    /// Forget the source so the next call asks for it again. Called when the
    /// surface answers nil, which is what a shut-down device looks like.
    private func forget() {
        surface = nil
        pixels = nil
        screen = nil
    }

    /// The encoder, kept between frames.
    ///
    /// Its width and height ARE the scale: a session smaller than the pixel
    /// buffer it is handed downscales, and the three `kVTScalingMode` values
    /// were measured to produce byte-identical output here, so none is set.
    /// (Setting `kVTPixelTransferPropertyKey_ScalingMode` directly on the
    /// session answers -12900 anyway; it belongs nested inside
    /// `PixelTransferProperties`, where it also changes nothing.)
    private func encoder(width: Int, height: Int, quality: Double) throws -> VTCompressionSession {
        if let session, sessionShape == (width, height, quality) { return session }
        if let session { VTCompressionSessionInvalidate(session) }
        session = nil
        var made: VTCompressionSession?
        let created = VTCompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            width: Int32(width),
            height: Int32(height),
            codecType: kCMVideoCodecType_JPEG,
            encoderSpecification: nil,
            imageBufferAttributes: nil,
            compressedDataAllocator: nil,
            outputCallback: nil,
            refcon: nil,
            compressionSessionOut: &made
        )
        guard created == noErr, let made else {
            throw HelperError.message("JPEG 인코더를 만들지 못했습니다: \(created)")
        }
        // Every VideoToolbox status is read. An ignored one is how a property
        // that does nothing gets written down as a property that works.
        let realTime = VTSessionSetProperty(
            made, key: kVTCompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        let asked = VTSessionSetProperty(
            made, key: kVTCompressionPropertyKey_Quality, value: quality as CFTypeRef)
        guard realTime == noErr, asked == noErr else {
            VTCompressionSessionInvalidate(made)
            throw HelperError.message("JPEG 인코더 설정에 실패했습니다: \(realTime)/\(asked)")
        }
        session = made
        sessionShape = (width, height, quality)
        return made
    }

    /// One picture, or nothing at all when the screen has not changed since the
    /// seed the caller last saw. Nothing is the common answer for a pane nobody
    /// is touching, and it costs a single shared-memory read.
    func capture(since: UInt32?, longEdge: Int, quality: Double) throws -> (Data, UInt32, Int, Int)? {
        turn.lock()
        defer { turn.unlock() }
        let held: IOSurfaceRef
        do {
            held = try liveSurface()
        } catch {
            forget()
            throw error
        }
        let seed = IOSurfaceGetSeed(held)
        if let since, since == seed { return nil }
        let sourceWidth = IOSurfaceGetWidth(held)
        let sourceHeight = IOSurfaceGetHeight(held)
        guard sourceWidth > 0, sourceHeight > 0 else {
            forget()
            throw HelperError.message("시뮬레이터 화면 크기가 0입니다")
        }
        let longest = max(sourceWidth, sourceHeight)
        let scale = longEdge > 0 && longEdge < longest ? Double(longEdge) / Double(longest) : 1.0
        let width = max(2, Int((Double(sourceWidth) * scale).rounded()))
        let height = max(2, Int((Double(sourceHeight) * scale).rounded()))
        guard let pixels else {
            forget()
            throw HelperError.message("시뮬레이터 픽셀 버퍼가 없습니다")
        }
        let session = try encoder(width: width, height: height, quality: quality)
        var produced: CMSampleBuffer?
        let encoded = VTCompressionSessionEncodeFrame(
            session,
            imageBuffer: pixels,
            presentationTimeStamp: CMTime(value: CMTimeValue(seed), timescale: 60),
            duration: .invalid,
            frameProperties: nil,
            infoFlagsOut: nil,
            outputHandler: { status, _, sample in
                if status == noErr { produced = sample }
            }
        )
        guard encoded == noErr else {
            throw HelperError.message("화면을 인코딩하지 못했습니다: \(encoded)")
        }
        let finished = VTCompressionSessionCompleteFrames(
            session, untilPresentationTimeStamp: .invalid)
        guard finished == noErr, let produced,
              let block = CMSampleBufferGetDataBuffer(produced)
        else {
            throw HelperError.message("인코딩된 화면을 받지 못했습니다: \(finished)")
        }
        var length = 0
        var pointer: UnsafeMutablePointer<Int8>?
        let read = CMBlockBufferGetDataPointer(
            block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &length,
            dataPointerOut: &pointer)
        guard read == noErr, let pointer, length > 0 else {
            throw HelperError.message("인코딩된 화면을 읽지 못했습니다: \(read)")
        }
        return (Data(bytes: pointer, count: length), seed, width, height)
    }
}

private final class HIDInjector {
    private typealias MouseFunction = @convention(c) (
        UnsafePointer<CGPoint>, UnsafePointer<CGPoint>?, UInt32, Int32, CGFloat, CGFloat, UInt32
    ) -> UnsafeMutableRawPointer?
    private typealias KeyboardFunction = @convention(c) (
        UInt32, UInt32
    ) -> UnsafeMutableRawPointer?
    private typealias ArbitraryHIDFunction = @convention(c) (
        UInt32, UInt32, UInt32, UInt32
    ) -> UnsafeMutableRawPointer?

    private let udid: String
    private let device: NSObject
    /// The same device object the frame source reads its screen from —
    /// looking it up twice would open a second CoreSimulator context.
    var simulator: NSObject { device }
    private let client: NSObject
    private let sendSelector = NSSelectorFromString(
        "sendWithMessage:freeWhenDone:completionQueue:completion:"
    )
    private let mouse: MouseFunction
    private let keyboard: KeyboardFunction
    private let arbitraryHID: ArbitraryHIDFunction?

    init(udid: String) throws {
        self.udid = udid
        try SimulatorFrameworks.load()
        self.device = try simulatorDevice(udid: udid)

        guard let mousePointer = dlsym(
            UnsafeMutableRawPointer(bitPattern: -2),
            "IndigoHIDMessageForMouseNSEvent"
        ) else {
            throw HelperError.message("시뮬레이터 터치 API를 찾지 못했습니다")
        }
        guard let keyboardPointer = dlsym(
            UnsafeMutableRawPointer(bitPattern: -2),
            "IndigoHIDMessageForKeyboardArbitrary"
        ) else {
            throw HelperError.message("시뮬레이터 키보드 API를 찾지 못했습니다")
        }
        self.mouse = unsafeBitCast(mousePointer, to: MouseFunction.self)
        self.keyboard = unsafeBitCast(keyboardPointer, to: KeyboardFunction.self)
        if let arbitraryPointer = dlsym(
            UnsafeMutableRawPointer(bitPattern: -2),
            "IndigoHIDMessageForHIDArbitrary"
        ) {
            self.arbitraryHID = unsafeBitCast(
                arbitraryPointer, to: ArbitraryHIDFunction.self
            )
        } else {
            self.arbitraryHID = nil
        }

        guard let clientClass = NSClassFromString(
            "_TtC12SimulatorKit24SimDeviceLegacyHIDClient"
        ) else {
            throw HelperError.message("SimulatorKit HID 클라이언트를 찾지 못했습니다")
        }
        let initialize = NSSelectorFromString("initWithDevice:error:")
        typealias InitializeFunction = @convention(c) (
            AnyObject, Selector, AnyObject, AutoreleasingUnsafeMutablePointer<NSError?>
        ) -> AnyObject?
        guard let implementation = class_getMethodImplementation(clientClass, initialize) else {
            throw HelperError.message("SimulatorKit HID 클라이언트를 초기화할 수 없습니다")
        }
        let initializeClient = unsafeBitCast(implementation, to: InitializeFunction.self)
        var error: NSError?
        guard let made = initializeClient(clientClass.alloc(), initialize, device, &error)
            as? NSObject else {
            throw error ?? HelperError.message("SimulatorKit HID 연결에 실패했습니다")
        }
        self.client = made
    }

    private func send(_ message: UnsafeMutableRawPointer?) throws {
        guard let message else {
            throw HelperError.message("시뮬레이터 HID 메시지를 만들지 못했습니다")
        }
        typealias SendFunction = @convention(c) (
            AnyObject, Selector, UnsafeMutableRawPointer, ObjCBool, AnyObject?, AnyObject?
        ) -> Void
        guard let isa = object_getClass(client),
              let implementation = class_getMethodImplementation(isa, sendSelector) else {
            free(message)
            throw HelperError.message("시뮬레이터 HID 메시지를 보낼 수 없습니다")
        }
        unsafeBitCast(implementation, to: SendFunction.self)(
            client, sendSelector, message, ObjCBool(true), nil, nil
        )
    }

    private func touch(_ type: String, x: Double, y: Double) throws {
        guard x.isFinite, y.isFinite else {
            throw HelperError.message("유효하지 않은 터치 좌표입니다")
        }
        let eventType: Int32
        switch type {
        case "begin", "move": eventType = 1
        case "end": eventType = 2
        default: throw HelperError.message("유효하지 않은 터치 단계입니다")
        }
        var point = CGPoint(x: min(max(x, 0), 1), y: min(max(y, 0), 1))
        try send(mouse(&point, nil, 0x32, eventType, 1, 1, 0))
    }

    private func multiTouch(
        _ type: String, x1: Double, y1: Double, x2: Double, y2: Double
    ) throws {
        guard [x1, y1, x2, y2].allSatisfy(\.isFinite) else {
            throw HelperError.message("유효하지 않은 다중 터치 좌표입니다")
        }
        let eventType: Int32
        switch type {
        case "begin", "move": eventType = 1
        case "end": eventType = 2
        default: throw HelperError.message("유효하지 않은 다중 터치 단계입니다")
        }
        var point1 = CGPoint(x: min(max(x1, 0), 1), y: min(max(y1, 0), 1))
        var point2 = CGPoint(x: min(max(x2, 0), 1), y: min(max(y2, 0), 1))
        try send(mouse(&point1, &point2, 0x32, eventType, 1, 1, 0))
    }

    func tap(x: Double, y: Double) throws {
        try touch("begin", x: x, y: y)
        usleep(16_000)
        try touch("end", x: x, y: y)
    }

    /// One phase of a touch somebody else is timing — the window streams the
    /// mouse as begin/move/end, so pacing and velocity are the person's own.
    func touchPhase(_ phase: String, x: Double, y: Double) throws {
        try touch(phase, x: x, y: y)
    }

    func swipe(
        x1: Double, y1: Double, x2: Double, y2: Double, durationMs: UInt32
    ) throws {
        let duration = min(max(durationMs, 50), 3_000)
        let frameMs: UInt32 = 16
        let steps = max(2, Int(duration / frameMs))
        try touch("begin", x: x1, y: y1)
        do {
            for step in 1..<steps {
                let progress = Double(step) / Double(steps)
                try touch(
                    "move",
                    x: x1 + (x2 - x1) * progress,
                    y: y1 + (y2 - y1) * progress
                )
                usleep(frameMs * 1_000)
            }
        } catch {
            // The finger MUST come up. A begin whose end is lost leaves the
            // OS holding a gesture forever — a half-pulled home swipe covers
            // the whole screen with its blur until something else touches it.
            try? touch("end", x: x2, y: y2)
            throw error
        }
        try touch("end", x: x2, y: y2)
    }

    func multiTouchGesture(
        x1: Double, y1: Double, x2: Double, y2: Double,
        x3: Double, y3: Double, x4: Double, y4: Double,
        durationMs: UInt32
    ) throws {
        let duration = min(max(durationMs, 50), 3_000)
        let frameMs: UInt32 = 16
        let steps = max(2, Int(duration / frameMs))
        try multiTouch("begin", x1: x1, y1: y1, x2: x2, y2: y2)
        do {
            usleep(frameMs * 1_000)
            for step in 1..<steps {
                let progress = Double(step) / Double(steps)
                try multiTouch(
                    "move",
                    x1: x1 + (x3 - x1) * progress,
                    y1: y1 + (y3 - y1) * progress,
                    x2: x2 + (x4 - x2) * progress,
                    y2: y2 + (y4 - y2) * progress
                )
                usleep(frameMs * 1_000)
            }
        } catch {
            // Same contract as the one-finger road: both fingers come up.
            try? multiTouch("end", x1: x3, y1: y3, x2: x4, y2: y4)
            throw error
        }
        try multiTouch("end", x1: x3, y1: y3, x2: x4, y2: y4)
    }

    private func key(_ usage: UInt32, down: Bool) throws {
        try send(keyboard(usage, down ? 1 : 2))
    }

    private func press(_ usage: UInt32, shift: Bool = false) throws {
        let leftShift: UInt32 = 0xE1
        if shift { try key(leftShift, down: true) }
        try key(usage, down: true)
        usleep(4_000)
        try key(usage, down: false)
        if shift { try key(leftShift, down: false) }
        usleep(4_000)
    }

    private func keySpec(for character: Character) -> (UInt32, Bool)? {
        let value = String(character)
        if value.count == 1, let ascii = value.utf8.first {
            if ascii >= 97 && ascii <= 122 { return (0x04 + UInt32(ascii - 97), false) }
            if ascii >= 65 && ascii <= 90 { return (0x04 + UInt32(ascii - 65), true) }
        }
        let digits = Array("1234567890")
        let shiftedDigits = Array("!@#$%^&*()")
        if let index = digits.firstIndex(of: character) { return (0x1E + UInt32(index), false) }
        if let index = shiftedDigits.firstIndex(of: character) { return (0x1E + UInt32(index), true) }
        let punctuation: [(Character, Character, UInt32)] = [
            ("-", "_", 0x2D), ("=", "+", 0x2E), ("[", "{", 0x2F),
            ("]", "}", 0x30), ("\\", "|", 0x31), (";", ":", 0x33),
            ("'", "\"", 0x34), ("`", "~", 0x35), (",", "<", 0x36),
            (".", ">", 0x37), ("/", "?", 0x38),
        ]
        for (plain, shifted, usage) in punctuation {
            if character == plain { return (usage, false) }
            if character == shifted { return (usage, true) }
        }
        if character == " " { return (0x2C, false) }
        return nil
    }

    func type(_ text: String) throws {
        for character in text {
            guard let (usage, shift) = keySpec(for: character) else {
                throw HelperError.message("US 키보드로 입력할 수 없는 문자입니다")
            }
            try press(usage, shift: shift)
        }
    }

    func paste() throws {
        let leftCommand: UInt32 = 0xE3
        let v: UInt32 = 0x19
        try key(leftCommand, down: true)
        try press(v)
        try key(leftCommand, down: false)
    }

    func button(_ name: String) throws {
        switch name {
        case "home":
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
            process.arguments = ["simctl", "launch", udid, "com.apple.springboard"]
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try process.run()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else {
                throw HelperError.message("홈 화면을 열지 못했습니다")
            }
        case "enter": try press(0x28)
        case "del": try press(0x2A)
        case "forward_del": try press(0x4C)
        case "escape": try press(0x29)
        case "tab": try press(0x2B)
        case "right": try press(0x4F)
        case "left": try press(0x50)
        case "down": try press(0x51)
        case "up": try press(0x52)
        case "power", "lock": try pressHardware(page: 12, usage: 48)
        case "volume-up", "volume_up", "volup":
            try pressHardware(page: 12, usage: 233)
        case "volume-down", "volume_down", "voldown":
            try pressHardware(page: 12, usage: 234)
        case "action": try pressHardware(page: 11, usage: 45)
        default: throw HelperError.message("알 수 없는 iOS 버튼입니다")
        }
    }

    private func pressHardware(page: UInt32, usage: UInt32) throws {
        guard let arbitraryHID else {
            throw HelperError.message("이 Xcode에는 하드웨어 버튼 주입 API가 없습니다")
        }
        try send(arbitraryHID(0x32, page, usage, 1))
        usleep(50_000)
        try send(arbitraryHID(0x32, page, usage, 2))
    }

    func rotate(_ rotation: UInt32) throws {
        // Window rotation values (portrait, left, upside-down, right) become
        // UIDeviceOrientation values expected by GraphicsServices.
        let orientation: UInt32
        switch rotation % 4 {
        case 1: orientation = 4
        case 2: orientation = 2
        case 3: orientation = 3
        default: orientation = 1
        }
        guard sendOrientation(orientation) else {
            throw HelperError.message("시뮬레이터를 회전하지 못했습니다")
        }
    }

    private func purpleWorkspacePort() -> mach_port_t {
        let lookupSelector = NSSelectorFromString("lookup:error:")
        typealias LookupFunction = @convention(c) (
            AnyObject, Selector, NSString, AutoreleasingUnsafeMutablePointer<NSError?>
        ) -> mach_port_t
        guard let isa = object_getClass(device),
              let implementation = class_getMethodImplementation(isa, lookupSelector) else {
            return 0
        }
        let lookup = unsafeBitCast(implementation, to: LookupFunction.self)
        var error: NSError?
        return lookup(device, lookupSelector, "PurpleWorkspacePort", &error)
    }

    private func ensureSimulatorApplication() {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        process.arguments = [
            "-gj", "-a", "Simulator", "--args", "-CurrentDeviceUDID", udid,
        ]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try? process.run()
        process.waitUntilExit()
    }

    private func sendOrientation(_ orientation: UInt32) -> Bool {
        let initialPort = purpleWorkspacePort()
        if initialPort != 0, deliverOrientation(orientation, to: initialPort) {
            return true
        }
        // A stopped Simulator.app can leave a stale non-zero send right in
        // CoreSimulator. Start it hidden and retry delivery, not just lookup.
        ensureSimulatorApplication()
        let pollIntervalMicroseconds: useconds_t = 100_000
        let maximumPolls = 20
        for _ in 0..<maximumPolls {
            usleep(pollIntervalMicroseconds)
            let port = purpleWorkspacePort()
            if port != 0, deliverOrientation(orientation, to: port) { return true }
        }
        return false
    }

    private func deliverOrientation(_ orientation: UInt32, to port: mach_port_t) -> Bool {
        var buffer = [UInt8](repeating: 0, count: 112)
        return buffer.withUnsafeMutableBufferPointer { pointer in
            guard let address = pointer.baseAddress else { return false }
            let base = UnsafeMutableRawPointer(address)
            let header = base.assumingMemoryBound(to: mach_msg_header_t.self)
            header.pointee.msgh_bits = mach_msg_bits_t(MACH_MSG_TYPE_COPY_SEND)
            header.pointee.msgh_size = 108
            header.pointee.msgh_remote_port = port
            header.pointee.msgh_local_port = mach_port_t(MACH_PORT_NULL)
            header.pointee.msgh_voucher_port = mach_port_t(MACH_PORT_NULL)
            header.pointee.msgh_id = 0x7B
            base.storeBytes(of: UInt32(50), toByteOffset: 0x18, as: UInt32.self)
            base.storeBytes(of: UInt32(0x20000), toByteOffset: 0x3C, as: UInt32.self)
            base.storeBytes(of: UInt32(4), toByteOffset: 0x48, as: UInt32.self)
            base.storeBytes(of: orientation, toByteOffset: 0x4C, as: UInt32.self)
            return mach_msg_send(header) == KERN_SUCCESS
        }
    }
}

/// What one request answers. Everything but a frame answers only `data`.
private struct Answer {
    var data: String?
    var seed: UInt32?
    var width: Int?
    var height: Int?
}

/// Dial the frame socket the caller bound before it spawned us.
///
/// Written against Darwin rather than Network.framework because the whole point
/// is a blocking file descriptor this process can write pictures to from its own
/// thread; an async connection with a queue and a state handler would put the
/// pictures back on somebody else's schedule.
private func connectUnixSocket(_ path: String) -> Int32? {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return nil }
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8)
    // `sun_path` is 104 bytes here and `connect` does not truncate politely.
    guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
        close(fd)
        return nil
    }
    withUnsafeMutablePointer(to: &address.sun_path) { tuple in
        tuple.withMemoryRebound(to: CChar.self, capacity: bytes.count + 1) { target in
            for (at, byte) in bytes.enumerated() { target[at] = CChar(bitPattern: byte) }
            target[bytes.count] = 0
        }
    }
    let joined = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { generic in
            Darwin.connect(fd, generic, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard joined == 0 else {
        close(fd)
        return nil
    }
    return fd
}

/// Write every byte or say it failed. A short write is normal on a socket and
/// treating one as success is how a reader ends up parsing a picture's tail as
/// the next picture's header.
private func writeAll(_ fd: Int32, _ data: Data) -> Bool {
    data.withUnsafeBytes { raw -> Bool in
        guard let base = raw.baseAddress else { return false }
        var sent = 0
        while sent < raw.count {
            let wrote = Darwin.write(fd, base.advanced(by: sent), raw.count - sent)
            if wrote > 0 {
                sent += wrote
                continue
            }
            if wrote < 0, errno == EINTR || errno == EAGAIN { continue }
            return false
        }
        return true
    }
}

/// The one frame source this process has, built on first use.
///
/// Lazily, and not at startup, for the reason the old inline closure gave: a
/// pane that only ever sends taps should not pay for an encoder, and a device
/// that is not booted should fail when a picture is WANTED rather than at the
/// door. A holder rather than a captured `var` because two threads now ask for
/// it and a plain variable would be built twice, or torn.
private final class FrameSourceHolder {
    private let device: NSObject
    private let turn = NSLock()
    private var source: FrameSource?

    init(device: NSObject) {
        self.device = device
    }

    func get() throws -> FrameSource {
        turn.lock()
        defer { turn.unlock() }
        if let source { return source }
        let made = FrameSource(device: device)
        source = made
        return made
    }
}

/// Pushes pictures down the frame socket for as long as it is asked to.
///
/// The reason this exists at all: answering a `frame` request costs a whole
/// round trip through this process's single request loop, so a pane drawing
/// sixty times a second made its own touches queue behind its own pictures —
/// and a pane on a still screen could only find out the screen had moved by
/// asking again, which is a poll, which is a delay. Here the surface's own
/// generation is watched and a picture goes out the moment it changes.
private final class FramePusher {
    private let socketPath: String?
    private let frames: FrameSourceHolder
    private let turn = NSLock()
    /// Bumped by every start and every stop, so a thread whose orders have been
    /// superseded notices and retires instead of racing its replacement.
    private var generation: UInt64 = 0
    private var running = false

    init(socketPath: String?, frames: FrameSourceHolder) {
        self.socketPath = socketPath
        self.frames = frames
    }

    func start(longEdge: Int, quality: Double, maxFps: UInt32) throws {
        guard let socketPath else {
            throw HelperError.message("이 헬퍼에는 화면 스트림 소켓이 없습니다")
        }
        // Built on the CALLER's thread so a device that cannot draw fails this
        // request, with its real reason, instead of dying quietly on a thread
        // nobody is reading.
        _ = try frames.get()
        turn.lock()
        generation &+= 1
        let mine = generation
        running = true
        turn.unlock()
        let interval = maxFps == 0 ? 0 : 1.0 / Double(maxFps)
        Thread.detachNewThread { [self] in
            pump(mine, socketPath, longEdge: longEdge, quality: quality, interval: interval)
        }
    }

    func stop() {
        turn.lock()
        generation &+= 1
        running = false
        turn.unlock()
    }

    private func current(_ mine: UInt64) -> Bool {
        turn.lock()
        defer { turn.unlock() }
        return running && generation == mine
    }

    private func pump(
        _ mine: UInt64, _ socketPath: String, longEdge: Int, quality: Double, interval: Double
    ) {
        guard let fd = connectUnixSocket(socketPath) else { return }
        defer { close(fd) }
        var seen: UInt32?
        while current(mine) {
            let started = Date()
            let picture: (Data, UInt32, Int, Int)?
            do {
                picture = try autoreleasepool {
                    try frames.get().capture(since: seen, longEdge: longEdge, quality: quality)
                }
            } catch {
                // The surface went away — the device shut down, or Xcode moved
                // under us. Closing the socket is how the reader is told.
                return
            }
            guard let picture else {
                Thread.sleep(forTimeInterval: FrameTuning.idlePoll)
                continue
            }
            seen = picture.1
            var head = Data(capacity: 16)
            for value in [UInt32(picture.0.count), picture.1, UInt32(picture.2), UInt32(picture.3)] {
                var big = value.bigEndian
                withUnsafeBytes(of: &big) { head.append(contentsOf: $0) }
            }
            guard writeAll(fd, head), writeAll(fd, picture.0) else { return }
            let spent = Date().timeIntervalSince(started)
            if interval > spent { Thread.sleep(forTimeInterval: interval - spent) }
        }
    }
}

private func handle(
    _ request: Request,
    with injector: HIDInjector,
    frames: FrameSourceHolder,
    pusher: FramePusher
) throws -> Answer {
    switch request.kind {
    case "ping": return Answer()
    case "touch":
        guard let phase = request.phase, ["begin", "move", "end"].contains(phase) else {
            throw HelperError.message("유효하지 않은 터치 단계입니다")
        }
        try injector.touchPhase(phase, x: request.x ?? .nan, y: request.y ?? .nan)
    case "tap":
        try injector.tap(x: request.x ?? .nan, y: request.y ?? .nan)
    case "swipe":
        try injector.swipe(
            x1: request.x1 ?? .nan,
            y1: request.y1 ?? .nan,
            x2: request.x2 ?? .nan,
            y2: request.y2 ?? .nan,
            durationMs: request.durationMs ?? 300
        )
    case "multitouch":
        try injector.multiTouchGesture(
            x1: request.x1 ?? .nan,
            y1: request.y1 ?? .nan,
            x2: request.x2 ?? .nan,
            y2: request.y2 ?? .nan,
            x3: request.x3 ?? .nan,
            y3: request.y3 ?? .nan,
            x4: request.x4 ?? .nan,
            y4: request.y4 ?? .nan,
            durationMs: request.durationMs ?? 300
        )
    case "text": try injector.type(request.text ?? "")
    case "paste": try injector.paste()
    case "button": try injector.button(request.name ?? "")
    case "rotate": try injector.rotate(request.rotation ?? 0)
    case "ax":
        let data = try AccessibilityBridge.shared.describeUI(udid: CommandLine.arguments[1])
        return Answer(data: String(decoding: data, as: UTF8.self))
    case "stream":
        try pusher.start(
            longEdge: request.longEdge ?? 0,
            quality: request.quality ?? FrameTuning.quality,
            maxFps: request.maxFps ?? FrameTuning.maxFps
        )
    case "streamstop":
        pusher.stop()
    case "frame":
        // `longEdge` decides the bytes. Nothing here has an opinion about how
        // big a pane is, so a caller that does not say gets the screen as it is.
        guard let picture = try frames.get().capture(
            since: request.seed,
            longEdge: request.longEdge ?? 0,
            quality: request.quality ?? FrameTuning.quality
        ) else {
            // Unchanged. The seed still comes back so the caller can keep
            // asking about the same picture without holding one of its own.
            let seed = request.seed
            return Answer(data: nil, seed: seed)
        }
        return Answer(
            data: picture.0.base64EncodedString(),
            seed: picture.1,
            width: picture.2,
            height: picture.3
        )
    default: throw HelperError.message("알 수 없는 입력 요청입니다")
    }
    return Answer()
}

private func write(_ response: Response) {
    guard let data = try? JSONEncoder().encode(response),
          let line = String(data: data, encoding: .utf8) else { return }
    print(line)
    fflush(stdout)
}

setbuf(stdout, nil)
setbuf(stderr, nil)
_ = NSApplication.shared
NSApplication.shared.setActivationPolicy(.accessory)

// The socket is optional: a caller that binds one gets pushed pictures, and one
// that does not keeps exactly the process it always had, answering `frame`
// requests one at a time. Both are supported so a machine where binding fails
// still mirrors, only slower.
guard CommandLine.arguments.count == 2 || CommandLine.arguments.count == 3 else {
    fputs("usage: zerocode-ios-emulator-input <simulator-udid> [frame-socket]\n", stderr)
    exit(2)
}

do {
    let injector = try HIDInjector(udid: CommandLine.arguments[1])
    let frames = FrameSourceHolder(device: injector.simulator)
    let pusher = FramePusher(
        socketPath: CommandLine.arguments.count == 3 ? CommandLine.arguments[2] : nil,
        frames: frames
    )
    while let line = readLine() {
        do {
            let request = try JSONDecoder().decode(Request.self, from: Data(line.utf8))
            let answer = try autoreleasepool {
                try handle(request, with: injector, frames: frames, pusher: pusher)
            }
            write(Response(
                id: request.id, ok: true, error: nil, data: answer.data,
                seed: answer.seed, width: answer.width, height: answer.height
            ))
        } catch {
            let request = try? JSONDecoder().decode(Request.self, from: Data(line.utf8))
            write(Response(
                id: request?.id ?? 0,
                ok: false,
                error: error.localizedDescription,
                data: nil
            ))
        }
    }
} catch {
    fputs("\(error.localizedDescription)\n", stderr)
    exit(1)
}
