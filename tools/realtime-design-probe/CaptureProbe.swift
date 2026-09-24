// Disposable own-process ScreenCaptureKit probe. Never enumerates other apps.
import AppKit
import ScreenCaptureKit
import CoreMedia
import CoreVideo
import Darwin

let timebase: Double = {
    var value = mach_timebase_info_data_t()
    mach_timebase_info(&value)
    return Double(value.numer) / Double(value.denom)
}()
func ns() -> Double { Double(mach_absolute_time()) * timebase }
func cpu() -> Double {
    var usage = rusage()
    getrusage(RUSAGE_SELF, &usage)
    return Double(usage.ru_utime.tv_sec + usage.ru_stime.tv_sec)
        + Double(usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) / 1e6
}
func idleSeconds() -> Double {
    let kinds: [CGEventType] = [.mouseMoved, .leftMouseDown, .rightMouseDown,
        .otherMouseDown, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged,
        .keyDown, .keyUp, .flagsChanged, .scrollWheel]
    return kinds.map { CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: $0) }.min() ?? 0
}

final class Canvas: NSView {
    var serial = 0
    override func draw(_ dirtyRect: NSRect) {
        NSColor(calibratedRed: serial % 2 == 0 ? 0.8 : 0.2,
                green: 0.35, blue: 0.2, alpha: 1).setFill()
        bounds.fill()
        NSColor.white.setFill()
        NSRect(x: 40 + serial % 600, y: 90, width: 20, height: 20).fill()
    }
}

final class Samples: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    let lock = NSLock()
    var rows: [[String: Any]] = []
    var latestDisplay: Double = 0
    var stage = -1
    var count = 0
    var postUntil: Double = 0
    var postStart: Double = 0
    var original = CGPoint.zero
    var owned = CGRect.zero
    var posted = 0
    var tracePointer = false
    var canPost = false
    var failure: String?

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        lock.lock(); failure = error.localizedDescription; lock.unlock()
    }
    func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
        let entered = ns()
        guard type == .screen, buffer.isValid,
              let list = CMSampleBufferGetSampleAttachmentsArray(buffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let info = list.first, let status = info[.status] as? Int,
              SCFrameStatus(rawValue: status) == .complete,
              let pixels = CMSampleBufferGetImageBuffer(buffer),
              let displayTicks = info[.displayTime] as? NSNumber else { return }
        let display = displayTicks.doubleValue * timebase
        // Fixed 64x64 ROI, a color decision, and CGEvent creation are timed.
        guard CVPixelBufferLockBaseAddress(pixels, .readOnly) == kCVReturnSuccess else { return }
        let stride = CVPixelBufferGetBytesPerRow(pixels)
        let width = CVPixelBufferGetWidth(pixels), height = CVPixelBufferGetHeight(pixels)
        var red = 0
        if let base = CVPixelBufferGetBaseAddress(pixels)?.assumingMemoryBound(to: UInt8.self) {
            for y in (height / 2)..<(height / 2 + min(64, height / 2)) {
                for x in (width / 2)..<(width / 2 + min(64, width / 2)) {
                    red += Int(base[y * stride + 4 * x + 2])
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(pixels, .readOnly)
        let warmColor = red > 4096 * 128
        let decided = ns()
        lock.lock()
        let active = canPost && postUntil > entered && entered >= postStart
        let p0 = original, region = owned, begin = postStart, end = postUntil
        let thisStage = stage
        lock.unlock()
        var point = p0
        if active {
            // Return to the original point before the end of this short interval.
            let t = min(1, (entered - begin) / max(1, end - begin - 100e6))
            point.x += (warmColor ? 30 : -30) * sin(t * 2 * .pi)
        }
        let event = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved,
                            mouseCursorPosition: point, mouseButton: .left)
        event?.setIntegerValueField(.eventSourceUserData, value: 0x52545052)
        var didPost = false
        if active, region.contains(point), let event {
            event.post(tap: .cghidEventTap)
            didPost = true
        }
        let ended = ns()
        lock.lock()
        latestDisplay = display
        count += 1
        if didPost { posted += 1 }
        rows.append(["kind": "frame", "stage": thisStage, "at_ns": entered,
                     "display_ns": display, "age_ms": (entered - display) / 1e6,
                     "color_ms": (decided - entered) / 1e6,
                     "tick_ms": (ended - entered) / 1e6, "red_sum": red,
                     "posted": didPost, "warm_color": warmColor, "width": width, "height": height])
        lock.unlock()
    }
    func append(_ row: [String: Any]) { lock.lock(); rows.append(row); lock.unlock() }
}

@MainActor final class Probe: NSObject, NSApplicationDelegate {
    let out: URL
    let movement: Bool
    let samples = Samples()
    let queue = DispatchQueue(label: "probe.capture")
    var window: NSWindow!
    var timer: Timer?
    var poller: Timer?
    var tap: CFMachPort?
    var tapSource: CFRunLoopSource?
    var bounds = CGRect.zero
    var p0 = CGPoint.zero

    init(out: URL, movement: Bool) { self.out = out; self.movement = movement }
    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { await run() }
    }
    func wait(_ seconds: Double) async { try? await Task.sleep(nanoseconds: UInt64(seconds * 1e9)) }
    func config(_ fps: Int) -> SCStreamConfiguration {
        let c = SCStreamConfiguration()
        c.width = 800; c.height = 500
        c.pixelFormat = kCVPixelFormatType_32BGRA
        c.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
        c.queueDepth = 4; c.showsCursor = false
        return c
    }
    func run() async {
        var rc: Int32 = 0
        var stream: SCStream?
        do {
            if CommandLine.arguments.contains("--movement-only") {
                let waiting = ns()
                while idleSeconds() < 3 && ns() - waiting < 60e9 { await wait(0.25) }
            }
            p0 = CGEvent(source: nil)?.location ?? .zero
            guard let screen = NSScreen.screens.first(where: {
                guard let id = $0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? UInt32 else { return false }
                return CGDisplayBounds(id).contains(p0)
            }), let id = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? UInt32
            else { throw NSError(domain: "probe", code: 1) }
            let display = CGDisplayBounds(id)
            bounds = CGRect(x: min(max(p0.x - 400, display.minX), display.maxX - 800),
                            y: min(max(p0.y - 250, display.minY), display.maxY - 500), width: 800, height: 500)
            let frame = NSRect(x: screen.frame.minX + bounds.minX - display.minX,
                               y: screen.frame.maxY - (bounds.maxY - display.minY), width: 800, height: 500)
            window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.title = "Owned realtime probe"
            let canvas = Canvas(frame: NSRect(x: 0, y: 0, width: 800, height: 500))
            window.contentView = canvas
            window.orderFrontRegardless() // Never activate or makeKey; keyboard focus stays untouched.
            timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 60, repeats: true) { _ in
                canvas.serial += 1; canvas.needsDisplay = true
            }
            await wait(1)
            // This API returns ONLY this process's windows (macOS 14.4+).
            let content = try await SCShareableContent.currentProcess
            guard let own = content.windows.first(where: { $0.windowID == UInt32(window.windowNumber) })
            else { throw NSError(domain: "own-window-unavailable", code: 2) }
            let s = SCStream(filter: SCContentFilter(desktopIndependentWindow: own), configuration: config(30), delegate: samples)
            try s.addStreamOutput(samples, type: .screen, sampleHandlerQueue: queue)
            let onlyMoves = CommandLine.arguments.contains("--movement-only")
            let baseCPU = cpu(), baseAt = ns()
            await wait(onlyMoves ? 0.2 : 8)
            samples.append(["kind": "baseline", "wall_s": (ns() - baseAt) / 1e9, "cpu_s": cpu() - baseCPU])
            try await s.startCapture(); stream = s
            poller = Timer.scheduledTimer(withTimeInterval: 0.004, repeats: true) { [samples] _ in
                samples.lock.lock()
                let at = ns()
                if samples.latestDisplay > 0 {
                    samples.rows.append(["kind": "poll", "stage": samples.stage,
                                         "at_ns": at, "age_ms": (at - samples.latestDisplay) / 1e6])
                }
                if samples.tracePointer, let point = CGEvent(source: nil)?.location {
                    samples.rows.append(["kind": "cursor_sample", "at_ns": at, "x": point.x, "y": point.y])
                }
                samples.lock.unlock()
            }
            let frequencies: [Int] = onlyMoves ? [] : [30, 60, 60, 30]
            for (index, fps) in frequencies.enumerated() {
                try await s.updateConfiguration(config(fps))
                samples.lock.withLock { samples.stage = -1 }
                await wait(2)
                let at = ns(), used = cpu()
                samples.lock.withLock { samples.stage = index }
                var load = [Double](repeating: 0, count: 3)
                getloadavg(&load, 3)
                await wait(12)
                samples.lock.withLock { samples.stage = -1 }
                samples.append(["kind": "arm", "stage": index, "fps": fps,
                    "wall_s": (ns() - at) / 1e9, "cpu_s": cpu() - used, "load": load])
            }
            if movement { await wait(0.3); try await movements(s) }
        } catch {
            rc = 1
            samples.append(["kind": "error", "message": String(describing: error)])
        }
        if let stream { try? await stream.stopCapture() }
        timer?.invalidate(); poller?.invalidate()
        if let tap { CGEvent.tapEnable(tap: tap, enable: false) }
        if let tapSource { CFRunLoopRemoveSource(CFRunLoopGetMain(), tapSource, .commonModes) }
        window?.close()
        let (rows, failure) = samples.lock.withLock { (samples.rows, samples.failure) }
        do {
            let bytes = try JSONSerialization.data(withJSONObject: ["rows": rows, "stream_error": failure as Any? ?? NSNull(), "rc": rc], options: [.sortedKeys])
            try bytes.write(to: out, options: .atomic)
        } catch { fputs("Cannot save probe: \(error)\n", stderr); rc = 2 }
        exit(rc)
    }

    func movements(_ stream: SCStream) async throws {
        // Requires explicit ledger permission before this flag is used.
        // Two intervals, total <5 s; no clicks, keys or scrolls; restore pointer.
        let owned = bounds.insetBy(dx: 50, dy: 50)
        let startedWaiting = ns()
        while idleSeconds() < 3 && ns() - startedWaiting < 60e9 { await wait(0.25) }
        guard idleSeconds() >= 3, let current = CGEvent(source: nil)?.location,
              owned.contains(current), owned.contains(CGPoint(x: current.x + 80, y: current.y)),
              owned.contains(CGPoint(x: current.x - 30, y: current.y)) else {
            samples.append(["kind": "movement_skipped", "reason": "not idle or cursor outside owned interior", "idle_s": idleSeconds(), "cursor_inside": CGEvent(source: nil).map { owned.contains($0.location) } ?? false])
            return
        }
        p0 = current
        samples.lock.withLock { samples.original = p0; samples.owned = owned; samples.canPost = CGPreflightPostEventAccess(); samples.tracePointer = true }
        // Listen only to moves inside the owned region; no keyboard event contents.
        let context = Unmanaged.passUnretained(samples).toOpaque()
        if !CommandLine.arguments.contains("--cli-only") {
        tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .tailAppendEventTap,
                               options: .listenOnly, eventsOfInterest: 1 << CGEventType.mouseMoved.rawValue,
                               callback: { _, _, event, context in
            guard let context else { return Unmanaged.passUnretained(event) }
            let s = Unmanaged<Samples>.fromOpaque(context).takeUnretainedValue()
            s.lock.lock()
            if s.owned.contains(event.location) {
                s.rows.append(["kind": "move_event", "at_ns": ns(), "event_ns": event.timestamp,
                    "x": event.location.x, "y": event.location.y,
                    "tag": event.getIntegerValueField(.eventSourceUserData)])
            }
            s.lock.unlock()
            return Unmanaged.passUnretained(event)
        }, userInfo: context)
        }
        if let tap {
            tapSource = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
            CFRunLoopAddSource(CFRunLoopGetMain(), tapSource, .commonModes)
            CGEvent.tapEnable(tap: tap, enable: true)
        }
        samples.append(["kind": "movement_begin", "tap_available": tap != nil, "post_access": CGPreflightPostEventAccess(), "idle_s": idleSeconds(), "at_ns": ns()])
        let began = ns()
        // The second command restores the original point; both use the public CLI.
        for (steps, target) in [(12, CGPoint(x: p0.x + 80, y: p0.y)), (24, p0)] {
            let at = ns()
            let result = await Task.detached { () -> [String: Any] in
                let child = Process()
                child.executableURL = URL(fileURLWithPath: "/usr/bin/env")
                child.arguments = ["zerocode-computer", "mouse-move", "--x", String(Double(target.x)), "--y", String(Double(target.y)), "--steps", String(steps), "--json"]
                let pipe = Pipe(); child.standardOutput = pipe; child.standardError = pipe
                do {
                    try child.run()
                    let timeout = DispatchWorkItem { if child.isRunning { child.terminate() } }
                    DispatchQueue.global().asyncAfter(deadline: .now() + 0.7, execute: timeout)
                    child.waitUntilExit(); timeout.cancel()
                    return ["rc": child.terminationStatus, "output": String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""]
                } catch { return ["rc": -1, "output": String(describing: error)] }
            }.value
            samples.append(["kind": "cli", "steps": steps, "start_ns": at, "end_ns": ns(), "result": result])
            if (result["rc"] as? Int32) != 0 { break }
        }
        // Return within the first interval even if the public CLI refused.
        if CGPreflightPostEventAccess() {
            CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: p0, mouseButton: .left)?.post(tap: .cghidEventTap)
        }
        samples.append(["kind": "movement_interval", "index": 0, "wall_ms": (ns() - began) / 1e6])
        samples.lock.withLock { samples.tracePointer = false }
        if CommandLine.arguments.contains("--cli-only") {
            await wait(0.03)
            if let final = CGEvent(source: nil)?.location {
                samples.append(["kind": "movement_restored", "distance_points": hypot(final.x - p0.x, final.y - p0.y)])
            }
            return
        }
        guard CGPreflightPostEventAccess() else {
            samples.append(["kind": "movement_skipped", "reason": "post permission absent; no permission prompt"])
            return
        }
        try await stream.updateConfiguration(config(60))
        await wait(3.2)
        guard idleSeconds() >= 3, let nowPoint = CGEvent(source: nil)?.location,
              hypot(nowPoint.x - p0.x, nowPoint.y - p0.y) < 2 else { return }
        let next = ns()
        samples.lock.withLock { samples.postStart = next; samples.postUntil = next + 500e6 }
        await wait(0.51)
        samples.lock.withLock { samples.postUntil = 0 }
        if CGPreflightPostEventAccess() {
            CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: p0, mouseButton: .left)?.post(tap: .cghidEventTap)
        }
        samples.append(["kind": "movement_interval", "index": 1, "wall_ms": (ns() - next) / 1e6])
        await wait(0.03)
        if let final = CGEvent(source: nil)?.location {
            samples.append(["kind": "movement_restored", "distance_points": hypot(final.x - p0.x, final.y - p0.y)])
        }
    }
}

@main struct Entry {
    @MainActor static func main() {
        guard CommandLine.arguments.count >= 2 else { fputs("CaptureProbe OUTPUT.json [--permitted-movement]\n", stderr); exit(64) }
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let probe = Probe(out: URL(fileURLWithPath: CommandLine.arguments[1]), movement: CommandLine.arguments.contains("--permitted-movement"))
        app.delegate = probe
        app.run()
    }
}
