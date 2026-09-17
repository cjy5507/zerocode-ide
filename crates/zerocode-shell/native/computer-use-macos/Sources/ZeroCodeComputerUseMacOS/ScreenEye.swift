import CoreMedia
import CoreVideo
import Foundation
import ScreenCaptureKit
import VideoToolbox
import ZeroCodeComputerUseMacOSCore

/// The continuous eye (docs/design/computer-use-full-operator.md §7.1, V1):
/// one ScreenCaptureKit stream of a display, kept while the display is being
/// looked at and delivered at the screenshot ladder's first rung — the GPU
/// scales it. Every repaint the window server reports is numbered with where
/// it fell (`EyeRing`); a desktop look answers the newest frame, encoded once
/// per repaint, instead of capturing the display. Every number comes from the
/// window's table with `eyeStart`.
final class ScreenEye: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var eyes: [CGDirectDisplayID: ScreenEye] = [:]
    /// Every eye opened gets the next number: a reopened eye numbers its
    /// repaints from nothing again, and an object's address can be reused.
    nonisolated(unsafe) private static var opened = 0
    /// How long starting the stream may take before it is refused.
    private static let streamTimeoutSeconds: TimeInterval = 5

    private let config: EyeConfig
    private let display: DesktopScreen.Display
    private let displayIndex: Int
    private let queue = DispatchQueue(label: "dev.zerocode.computer-use.eye")
    /// The idle timer's own queue: closing the eye waits for the stream to
    /// stop, which must never happen on the queue the stream delivers on.
    private static let timers = DispatchQueue(label: "dev.zerocode.computer-use.eye.idle")
    private let stateLock = NSLock()
    private let firstFrame = DispatchSemaphore(value: 0)
    private var ring: EyeRing
    private var stream: SCStream?
    private var failure: String?
    // The newest frame and the repaint it came with; its answer, once encoded.
    private var newest: CVPixelBuffer?
    private var newestSeq = 0
    private var encoded: (seq: Int, answer: [String: Any])?
    private var sawFrame = false
    private var readGeneration = 0
    /// The repaints' units per point, learned from the first frame.
    private var unitsPerPoint: CGFloat?

    /// Which eye this is, among every eye this helper opened.
    private let generation: Int
    /// Survives repeated reads, changes on reopen and across helper restarts.
    private let streamId = UUID().uuidString

    private init(config: EyeConfig, display: DesktopScreen.Display, displayIndex: Int) {
        Self.lock.lock()
        Self.opened += 1
        self.generation = Self.opened
        Self.lock.unlock()
        self.config = config
        self.display = display
        self.displayIndex = displayIndex
        self.ring = EyeRing(config: config)
    }

    // MARK: the requests

    /// Keep an eye on display `index` (the order `--display N` names), with
    /// the window's table; one already open with the same table is kept.
    static func start(config: EyeConfig, displayIndex: Int?) throws -> [String: Any] {
        guard screenCaptureTrusted() else {
            throw ProviderError.coded(
                "permission_denied",
                "the eye uses the Screen & System Audio Recording permission the screenshots use (zerocode-computer permissions --id screenshots)"
            )
        }
        let (display, index) = try chosen(displayIndex)
        lock.lock()
        let running = eyes[display.id]
        lock.unlock()
        if let running, running.config == config, running.alive {
            running.touch()
            return running.state(after: nil, fromMs: nil)
        }
        if let running { stop(ifCurrent: running) }
        let eye = ScreenEye(config: config, display: display, displayIndex: index)
        try eye.begin()
        lock.lock()
        eyes[display.id] = eye
        lock.unlock()
        eye.touch()
        return eye.state(after: nil, fromMs: nil)
    }

    /// Where the eye on display `index` stands, and — when a cursor or a
    /// moment is named — the repaints after it.
    static func changes(displayIndex: Int?, after: Int?, fromMs: Int64?) throws -> [String: Any] {
        let eye = try running(displayIndex)
        eye.touch()
        return eye.state(after: after, fromMs: fromMs)
    }

    /// The newest frame of display `index`, as a desktop look answers it.
    static func frame(displayIndex: Int?) throws -> [String: Any] {
        let eye = try running(displayIndex)
        eye.touch()
        return try eye.answer()
    }

    /// Where an open eye stands: which eye (a reopened one is another), its
    /// newest repaint, and its table.
    struct Standing {
        let generation: Int
        let seq: Int
        let config: EyeConfig
    }

    /// Where every open eye stands, by display — read before pixels are
    /// taken, so a repaint after it is one those pixels may have missed.
    static func standing() -> [CGDirectDisplayID: Standing] {
        lock.lock()
        let open = eyes
        lock.unlock()
        return open.compactMapValues { eye in
            guard eye.alive else { return nil }
            eye.stateLock.lock()
            defer { eye.stateLock.unlock() }
            return Standing(generation: eye.generation, seq: eye.ring.latest, config: eye.config)
        }
    }

    /// Where the eye `generation` saw the display repaint after `seq` (screen
    /// points), and whether none of those was dropped: nil when that eye is
    /// no longer the one open. A read keeps the eye open.
    static func repaints(display id: CGDirectDisplayID, generation: Int, after seq: Int) -> (rects: [CGRect], whole: Bool)? {
        lock.lock()
        let eye = eyes[id]
        lock.unlock()
        guard let eye, eye.generation == generation, eye.alive else { return nil }
        eye.touch()
        eye.stateLock.lock()
        defer { eye.stateLock.unlock() }
        let asked = eye.ring.changes(after: seq, fromMs: 0)
        return (asked.changes.flatMap(\.rects), asked.whole)
    }

    /// An act begins: every open eye marks it, whichever display it changes.
    static func markAct() {
        lock.lock()
        let open = Array(eyes.values)
        lock.unlock()
        let atMs = nowMs()
        for eye in open {
            eye.stateLock.lock()
            eye.ring.markAct(atMs: atMs)
            eye.stateLock.unlock()
        }
    }

    private static func chosen(_ displayIndex: Int?) throws -> (DesktopScreen.Display, Int) {
        let all = DesktopScreen.displays()
        let index = displayIndex ?? 0
        guard all.indices.contains(index) else {
            throw ProviderError.coded("invalid_argument", "display \(index) is not one of the \(all.count) active displays")
        }
        return (all[index], index)
    }

    private static func running(_ displayIndex: Int?) throws -> ScreenEye {
        let (display, _) = try chosen(displayIndex)
        lock.lock()
        let eye = eyes[display.id]
        lock.unlock()
        // A display that changed its size or scale since the eye opened is
        // seen at the wrong size and its repaints land in the wrong points.
        if let eye, eye.display.bounds != display.bounds || eye.display.scale != display.scale {
            stop(ifCurrent: eye)
            throw ProviderError.coded("not_watching", "the display changed since the eye opened")
        }
        guard let eye, eye.alive else {
            throw ProviderError.coded("not_watching", "no eye is open on that display — the window starts one with eyeStart")
        }
        return eye
    }

    private static func stop(ifCurrent eye: ScreenEye) {
        lock.lock()
        let same = eyes[eye.display.id] === eye
        if same { eyes[eye.display.id] = nil }
        lock.unlock()
        if same { eye.end() }
    }

    // MARK: the stream

    private var alive: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return stream != nil && failure == nil
    }

    private func begin() throws {
        let size = EyeFrameSize.of(
            pixelWidth: Int((display.bounds.width * display.scale).rounded()),
            pixelHeight: Int((display.bounds.height * display.scale).rounded())
        )
        let config = self.config
        let id = display.id
        let stream = try BlockingAsync.run(timeout: Self.streamTimeoutSeconds, what: "opening the eye") { [self] in
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            guard let shown = content.displays.first(where: { $0.displayID == id }) else {
                throw ProviderError.coded("unsupported_capability", "ScreenCaptureKit does not show that display")
            }
            let filter = SCContentFilter(display: shown, excludingApplications: [], exceptingWindows: [])
            let configuration = SCStreamConfiguration()
            configuration.width = size.width
            configuration.height = size.height
            configuration.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(config.framesPerSecond))
            configuration.pixelFormat = kCVPixelFormatType_32BGRA
            // The screenshots never showed the pointer; neither does the eye,
            // and a pointer moving is not the screen changing.
            configuration.showsCursor = false
            configuration.queueDepth = 4
            let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
            try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
            try await stream.startCapture()
            return stream
        }
        stateLock.lock()
        self.stream = stream
        stateLock.unlock()
        // A look right after the start answers a frame, not an empty eye.
        if firstFrame.wait(timeout: .now() + .milliseconds(Int(config.firstFrameMs))) == .timedOut {
            end()
            throw ProviderError.coded("action_timeout", "the eye's first frame did not come in \(config.firstFrameMs) ms")
        }
    }

    private func end() {
        stateLock.lock()
        let stream = self.stream
        self.stream = nil
        newest = nil
        encoded = nil
        stateLock.unlock()
        if let stream {
            let handle = EyeStreamHandle(stream: stream)
            _ = try? BlockingAsync.run(timeout: Self.streamTimeoutSeconds, what: "closing the eye") {
                try await handle.stream.stopCapture()
            }
        }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sampleBuffer.isValid,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let info = attachments.first,
              let raw = info[.status] as? Int,
              SCFrameStatus(rawValue: raw) == .complete,
              let pixels = CMSampleBufferGetImageBuffer(sampleBuffer)
        else { return }
        let width = CVPixelBufferGetWidth(pixels)
        let dirty = Self.rects(info[.dirtyRects])
        let atMs = Self.nowMs()
        stateLock.lock()
        let per = unitsPerPoint ?? EyeFrameSize.unitsPerPoint(
            firstExtent: dirty.map(\.maxX).max(),
            frameWidth: width,
            displayBounds: display.bounds,
            displayScale: display.scale
        )
        unitsPerPoint = per
        // A frame that names nothing changed everywhere.
        let rects = dirty.isEmpty
            ? [display.bounds]
            : dirty.map { EyeFrameSize.onScreen($0, unitsPerPoint: per, displayBounds: display.bounds) }
        // The stream can repeat a whole-display invalidation at startup.
        // Only identical pixels are a repeat: time since opening says
        // nothing about whether a real scene transition happened.
        if EyePixels.isRepaint(newest, pixels) {
            newestSeq = ring.note(rects: rects, atMs: atMs).seq
        }
        newest = pixels
        let first = !sawFrame
        sawFrame = true
        stateLock.unlock()
        if first { firstFrame.signal() }
    }

    /// The repaints of a frame: CGRects in NSValues, as the header says, or
    /// the dictionaries a CF attachment carries them in.
    private static func rects(_ raw: Any?) -> [CGRect] {
        (raw as? [Any] ?? []).compactMap { element in
            if let value = element as? NSValue { return value.rectValue }
            if let dictionary = element as? NSDictionary { return CGRect(dictionaryRepresentation: dictionary) }
            return nil
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        stateLock.lock()
        failure = "the eye's stream stopped: \(error.localizedDescription)"
        stateLock.unlock()
    }

    // MARK: answers

    /// The newest frame as a desktop screenshot answers it — the same shape
    /// (`DesktopScreen.capture`), plus the repaint it shows. Encoded once per
    /// repaint: a look at a screen that has not changed costs no encode.
    private func answer() throws -> [String: Any] {
        stateLock.lock()
        let held = newest
        let seq = newestSeq
        let cached = encoded
        stateLock.unlock()
        if let cached, cached.seq == seq { return cached.answer }
        guard let held else {
            throw ProviderError.coded("not_watching", "the eye has no frame yet")
        }
        var image: CGImage?
        let made = VTCreateCGImageFromCVPixelBuffer(held, options: nil, imageOut: &image)
        guard made == noErr, let image, let png = boundedPngData(image) else {
            throw ProviderError.coded("accessibility_error", "encoding the eye's frame failed")
        }
        let answer: [String: Any] = [
            "screenshot": [
                "data": png.data.base64EncodedString(),
                "width": png.width,
                "height": png.height,
                "scale": desktopScreenshotScale(pixelWidth: png.width, pointsWidth: display.bounds.width),
            ],
            "origin": ["x": display.bounds.minX, "y": display.bounds.minY],
            "display": DesktopScreen.render(display, index: displayIndex),
            "seq": seq,
            "streamId": streamId,
        ]
        stateLock.lock()
        encoded = (seq, answer)
        stateLock.unlock()
        return answer
    }

    private func state(after: Int?, fromMs: Int64?) -> [String: Any] {
        stateLock.lock()
        defer { stateLock.unlock() }
        var state: [String: Any] = [
            "streaming": stream != nil && failure == nil,
            "seq": ring.latest,
            "streamId": streamId,
            "nowMs": Self.nowMs(),
            "display": DesktopScreen.render(display, index: displayIndex),
        ]
        if let act = ring.act { state["act"] = ["seq": act.seq, "atMs": act.atMs] }
        if let unitsPerPoint { state["unitsPerPoint"] = unitsPerPoint }
        if let failure { state["failure"] = failure }
        if after != nil || fromMs != nil {
            let asked = ring.changes(after: after ?? 0, fromMs: fromMs ?? 0)
            state["whole"] = asked.whole
            state["changes"] = asked.changes.map { change in
                [
                    "seq": change.seq,
                    "atMs": change.atMs,
                    "rects": change.rects.map { [$0.minX, $0.minY, $0.width, $0.height] },
                ] as [String: Any]
            }
        }
        return state
    }

    /// A read keeps the eye open; nobody reading for the table's idle time
    /// closes it — and with it the system's recording indicator.
    private func touch() {
        stateLock.lock()
        readGeneration += 1
        let generation = readGeneration
        stateLock.unlock()
        Self.timers.asyncAfter(deadline: .now() + .milliseconds(Int(config.idleStopMs))) { [weak self] in
            guard let self else { return }
            self.stateLock.lock()
            let idle = self.readGeneration == generation
            self.stateLock.unlock()
            if idle { Self.stop(ifCurrent: self) }
        }
    }

    private static func nowMs() -> Int64 {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }
}

/// A stream handed to the task that stops it; the eye's lock is what keeps
/// it from being used twice.
private struct EyeStreamHandle: @unchecked Sendable {
    let stream: SCStream
}
