import CoreMedia
import CoreVideo
import Darwin
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
///
/// A reflex run reads the same stream (realtime v1 §5.1): it is a reader that
/// asks for its own rate — the stream's rate changes in place, never a restart,
/// so the repaint numbers, the encoded look and every other reader's cursor
/// carry on — keeps the stream open past the idle time, and is woken on every
/// capture with the newest frame's facts (`ReflexCaptureBook`), a frame that
/// was not a capture of the display included.
final class ScreenEye: NSObject, SCStreamOutput, SCStreamDelegate, EyeFeed, @unchecked Sendable {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var eyes: [CGDirectDisplayID: ScreenEye] = [:]
    /// Every eye opened gets the next number: a reopened eye numbers its
    /// repaints from nothing again, and an object's address can be reused.
    nonisolated(unsafe) private static var opened = 0
    /// How long starting the stream may take before it is refused.
    private static let streamTimeoutSeconds: TimeInterval = 5
    /// Host ticks to nanoseconds, for the display time a frame carries.
    private static let timebase: mach_timebase_info_data_t = {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        return info
    }()

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
    /// The window's table and the runs reading beside it.
    private var readers = EyeReaders()
    /// Every frame the stream delivered, as a capture a run reads.
    private var book: ReflexCaptureBook
    private var wakes: [String: @Sendable () -> Void] = [:]

    /// Which eye this is, among every eye this helper opened.
    private let generation: Int
    /// Survives repeated reads, changes on reopen and across helper restarts.
    private let streamId = UUID().uuidString

    private init(config: EyeConfig, display: DesktopScreen.Display, geometry: EyeGeometry, displayIndex: Int) {
        Self.lock.lock()
        Self.opened += 1
        self.generation = Self.opened
        Self.lock.unlock()
        self.config = config
        self.display = display
        self.displayIndex = displayIndex
        self.ring = EyeRing(config: config)
        self.book = ReflexCaptureBook(opened: geometry, generation: UInt64(generation))
        super.init()
        _ = readers.start(config)
    }

    // MARK: the requests

    /// Keep an eye on display `index` (the order `--display N` names), with
    /// the window's table; one already open with the same table is kept —
    /// whatever rate a run reading it asked for.
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
        if let running, running.alive, running.table(config) == .keep {
            running.touch()
            return running.state(after: nil, fromMs: nil)
        }
        if let running { stop(ifCurrent: running) }
        guard let geometry = DesktopScreen.geometry(of: display.id) else {
            throw ProviderError.coded("not_watching", "display \(index) is gone")
        }
        let eye = ScreenEye(config: config, display: display, geometry: geometry, displayIndex: index)
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

    /// A run reads display `index` at `framesPerSecond` (the window's reflex
    /// table) beside the window's own looks: the eye is opened with the
    /// window's eye table when it is not, and its rate and colour space change
    /// in place.
    static func reader(_ reader: String, config: EyeConfig, displayIndex: Int, framesPerSecond: Int) throws -> ReflexEyeReader {
        _ = try start(config: config, displayIndex: displayIndex)
        let eye = try running(displayIndex)
        try eye.add(reader, framesPerSecond: framesPerSecond)
        return ReflexEyeReader(eye: eye, reader: reader)
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
        // A display that changed its place, size, scale or turn since the eye
        // opened is seen at the wrong size and its repaints land in the wrong
        // points — the same word a run's reader reads before every frame.
        if let eye, !eye.describes(DesktopScreen.geometry(of: display.id)) {
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

    /// Whether this eye is still the one open on its display.
    private static func isOpen(_ eye: ScreenEye) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return eyes[eye.display.id] === eye
    }

    /// Whether the display, standing at `now`, is still the one this eye
    /// opened on: the same place, size, scale and turn.
    private func describes(_ now: EyeGeometry?) -> Bool {
        now == book.opened
    }

    // MARK: the stream

    private var alive: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return stream != nil && failure == nil
    }

    /// What the window's table means for this eye: kept, or another eye.
    private func table(_ table: EyeConfig) -> EyeReaders.Change {
        stateLock.lock()
        defer { stateLock.unlock() }
        var probe = readers
        return probe.start(table)
    }

    /// The stream's settings: the ladder's size, the rate every reader
    /// together asks for, and sRGB while a run reads colours from it.
    private static func configuration(width: Int, height: Int, framesPerSecond: Int, srgb: Bool) -> SCStreamConfiguration {
        let configuration = SCStreamConfiguration()
        configuration.width = width
        configuration.height = height
        configuration.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(framesPerSecond))
        configuration.pixelFormat = kCVPixelFormatType_32BGRA
        // The screenshots never showed the pointer; neither does the eye,
        // and a pointer moving is not the screen changing.
        configuration.showsCursor = false
        configuration.queueDepth = 4
        if srgb { configuration.colorSpaceName = CGColorSpace.sRGB }
        return configuration
    }

    private var size: (width: Int, height: Int) {
        EyeFrameSize.of(
            pixelWidth: Int((display.bounds.width * display.scale).rounded()),
            pixelHeight: Int((display.bounds.height * display.scale).rounded())
        )
    }

    private func begin() throws {
        let size = self.size
        let framesPerSecond = config.framesPerSecond
        let id = display.id
        let stream = try BlockingAsync.run(timeout: Self.streamTimeoutSeconds, what: "opening the eye") { [self] in
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            guard let shown = content.displays.first(where: { $0.displayID == id }) else {
                throw ProviderError.coded("unsupported_capability", "ScreenCaptureKit does not show that display")
            }
            let filter = SCContentFilter(display: shown, excludingApplications: [], exceptingWindows: [])
            let configuration = Self.configuration(width: size.width, height: size.height, framesPerSecond: framesPerSecond, srgb: false)
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
        // A run reading this eye hears it close, and its next read is a
        // capture it refuses (`ReflexCaptureBook.read`).
        let woken = Array(wakes.values)
        wakes.removeAll()
        readers.closed()
        stateLock.unlock()
        for wake in woken { wake() }
        if let stream {
            let handle = EyeStreamHandle(stream: stream)
            _ = try? BlockingAsync.run(timeout: Self.streamTimeoutSeconds, what: "closing the eye") {
                try await handle.stream.stopCapture()
            }
        }
    }

    /// A run reads at `framesPerSecond`: the rate goes up in place if it is
    /// more than the stream runs at, and the stream delivers sRGB while any
    /// run reads it.
    private func add(_ reader: String, framesPerSecond: Int) throws {
        stateLock.lock()
        let wasHeld = readers.held
        let change = readers.add(reader, framesPerSecond: framesPerSecond)
        let rate = readers.framesPerSecond ?? config.framesPerSecond
        stateLock.unlock()
        guard change != .keep || !wasHeld else { return }
        do {
            try reconfigure(framesPerSecond: rate, srgb: true)
        } catch {
            stateLock.lock()
            _ = readers.remove(reader)
            stateLock.unlock()
            throw error
        }
    }

    /// A run is done reading: its rate and its colour space are given back in
    /// place, and it is woken no more.
    func remove(_ reader: String) {
        stateLock.lock()
        wakes[reader] = nil
        let change = readers.remove(reader)
        let held = readers.held
        let rate = readers.framesPerSecond ?? config.framesPerSecond
        stateLock.unlock()
        if change != .keep || !held {
            try? reconfigure(framesPerSecond: rate, srgb: held)
        }
    }

    func wake(_ reader: String, _ wake: (@Sendable () -> Void)?) {
        stateLock.lock()
        wakes[reader] = wake
        stateLock.unlock()
    }

    private func reconfigure(framesPerSecond: Int, srgb: Bool) throws {
        stateLock.lock()
        let stream = self.stream
        stateLock.unlock()
        guard let stream else { throw ProviderError.coded("not_watching", "the eye closed") }
        let handle = EyeStreamHandle(stream: stream)
        let size = self.size
        try BlockingAsync.run(timeout: Self.streamTimeoutSeconds, what: "changing the eye's rate") {
            try await handle.stream.updateConfiguration(
                Self.configuration(width: size.width, height: size.height, framesPerSecond: framesPerSecond, srgb: srgb)
            )
        }
    }

    /// The newest frame and its capture facts, for a run's evaluating thread,
    /// read with the display as it stands now and whether this eye is still
    /// the one open on it (`ReflexCaptureBook.read`).
    func readerCapture() -> (pixels: CVPixelBuffer?, capture: ReflexCapture)? {
        let now = DesktopScreen.geometry(of: display.id)
        let open = Self.isOpen(self)
        stateLock.lock()
        defer { stateLock.unlock() }
        guard let capture = book.read(open: open && stream != nil && failure == nil, now: now) else { return nil }
        return (newest, capture)
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        let deliveredNs = DispatchTime.now().uptimeNanoseconds
        guard type == .screen, sampleBuffer.isValid,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let info = attachments.first,
              let raw = info[.status] as? Int,
              let status = SCFrameStatus(rawValue: raw)
        else { return }
        let capturedNs = (info[.displayTime] as? UInt64).map(Self.nanoseconds)
        switch status {
        case .complete:
            guard let pixels = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
            deliver(pixels, info: info, capturedNs: capturedNs, deliveredNs: deliveredNs)
        case .idle:
            // Nothing changed: a new capture of the pixels already held.
            idle(capturedNs: capturedNs, deliveredNs: deliveredNs)
        default:
            // Blank, suspended, starting, stopping: the newest capture no
            // longer shows the display, and a run reading it is told now.
            stateLock.lock()
            let woken = book.interrupted(capturedNs: capturedNs, deliveredNs: deliveredNs) == nil ? [] : Array(wakes.values)
            stateLock.unlock()
            for wake in woken { wake() }
        }
    }

    private func deliver(_ pixels: CVPixelBuffer, info: [SCStreamFrameInfo: Any], capturedNs: UInt64?, deliveredNs: UInt64) {
        let width = CVPixelBufferGetWidth(pixels)
        let height = CVPixelBufferGetHeight(pixels)
        let dirty = Self.rects(info[.dirtyRects])
        let atMs = Self.nowMs()
        let space = Self.colorSpace(pixels)
        let now = DesktopScreen.geometry(of: display.id)
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
        let woken = captured(width: width, height: height, space: space, dirty: !dirty.isEmpty, capturedNs: capturedNs, deliveredNs: deliveredNs, now: now)
        stateLock.unlock()
        if first { firstFrame.signal() }
        for wake in woken { wake() }
    }

    private func idle(capturedNs: UInt64?, deliveredNs: UInt64) {
        let now = DesktopScreen.geometry(of: display.id)
        stateLock.lock()
        guard let held = newest, let last = book.newest else {
            stateLock.unlock()
            return
        }
        let woken = captured(
            width: CVPixelBufferGetWidth(held),
            height: CVPixelBufferGetHeight(held),
            space: last.colorSpace,
            dirty: false,
            capturedNs: capturedNs,
            deliveredNs: deliveredNs,
            now: now
        )
        stateLock.unlock()
        for wake in woken { wake() }
    }

    /// Note one capture (under `stateLock`) with the display as it stood when
    /// it came, and answer whom to wake.
    private func captured(width: Int, height: Int, space: ReflexColorSpace, dirty: Bool, capturedNs: UInt64?, deliveredNs: UInt64, now: EyeGeometry?) -> [@Sendable () -> Void] {
        book.delivered(
            extent: ReflexPixelExtent(width: UInt32(clamping: width), height: UInt32(clamping: height)),
            colorSpace: space,
            dirty: dirty,
            repaintSeq: UInt64(ring.latest),
            capturedNs: capturedNs,
            deliveredNs: deliveredNs,
            now: now
        )
        return Array(wakes.values)
    }

    private static func nanoseconds(_ ticks: UInt64) -> UInt64 {
        let product = ticks.multipliedFullWidth(by: UInt64(timebase.numer))
        return UInt64(timebase.denom).dividingFullWidth(product).quotient
    }

    /// The colour space a frame's pixels are in, when the buffer says; unknown
    /// otherwise — never assumed.
    private static func colorSpace(_ pixels: CVPixelBuffer) -> ReflexColorSpace {
        let space = CVImageBufferGetColorSpace(pixels)?.takeUnretainedValue()
            ?? CVBufferCopyAttachments(pixels, .shouldPropagate).flatMap { CVImageBufferCreateColorSpaceFromAttachments($0)?.takeRetainedValue() }
        guard let name = space?.name else { return .unknown }
        if name == CGColorSpace.sRGB { return .srgb }
        if name == CGColorSpace.displayP3 { return .display_p3 }
        return .unknown
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
        let woken = Array(wakes.values)
        stateLock.unlock()
        for wake in woken { wake() }
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
        if let rate = readers.framesPerSecond, rate != config.framesPerSecond { state["framesPerSecond"] = rate }
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
    /// closes it — and with it the system's recording indicator — unless a
    /// run is reading it.
    private func touch() {
        stateLock.lock()
        readGeneration += 1
        let generation = readGeneration
        stateLock.unlock()
        Self.timers.asyncAfter(deadline: .now() + .milliseconds(Int(config.idleStopMs))) { [weak self] in
            guard let self else { return }
            self.stateLock.lock()
            let idle = self.readGeneration == generation && !self.readers.held
            self.stateLock.unlock()
            if idle { Self.stop(ifCurrent: self) }
        }
    }

    private static func nowMs() -> Int64 {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }
}

/// What a run's reader asks of an eye (`ScreenEye`): its newest capture, read
/// against the display as it stands, the run's wake, and its leaving.
protocol EyeFeed: AnyObject, Sendable {
    func readerCapture() -> (pixels: CVPixelBuffer?, capture: ReflexCapture)?
    func wake(_ reader: String, _ wake: (@Sendable () -> Void)?)
    func remove(_ reader: String)
}

/// A run's hold on one display's eye: the frame source its evaluating thread
/// reads. Every read asks the eye whether its capture still describes the
/// display — the eye still the one open on it, at the place, size, scale and
/// turn it opened with — so a display that changed hands the run a capture it
/// refuses, never the old point transform. Ending it gives the run's rate
/// back; nothing restarts.
final class ReflexEyeReader: ReflexRunSource, @unchecked Sendable {
    private let eye: any EyeFeed
    private let reader: String
    private let lock = NSLock()
    private var ended = false

    init(eye: any EyeFeed, reader: String) {
        self.eye = eye
        self.reader = reader
    }

    func newest() -> ReflexFrameLoan? {
        guard let (pixels, capture) = eye.readerCapture() else { return nil }
        return ReflexFrameLoan(capture: capture) { body in
            // Only a ready capture's buffer is lent, and only the one its
            // facts describe: BGRA, the extent they name.
            guard capture.status == .ready, let pixels,
                  CVPixelBufferGetPixelFormatType(pixels) == kCVPixelFormatType_32BGRA,
                  CVPixelBufferGetWidth(pixels) == Int(capture.pixelExtent.width),
                  CVPixelBufferGetHeight(pixels) == Int(capture.pixelExtent.height),
                  CVPixelBufferLockBaseAddress(pixels, .readOnly) == kCVReturnSuccess
            else { return false }
            defer { CVPixelBufferUnlockBaseAddress(pixels, .readOnly) }
            guard let base = CVPixelBufferGetBaseAddress(pixels) else { return false }
            let bytesPerRow = CVPixelBufferGetBytesPerRow(pixels)
            guard bytesPerRow >= CVPixelBufferGetWidth(pixels) * 4 else { return false }
            body(ReflexPixels(base: UnsafeRawPointer(base), width: CVPixelBufferGetWidth(pixels), height: CVPixelBufferGetHeight(pixels), bytesPerRow: bytesPerRow))
            return true
        }
    }

    func onCapture(_ wake: (@Sendable () -> Void)?) {
        eye.wake(reader, wake)
    }

    /// Give the run's rate back, once.
    func end() {
        lock.lock()
        let first = !ended
        ended = true
        lock.unlock()
        if first { eye.remove(reader) }
    }
}

/// A stream handed to the task that stops it; the eye's lock is what keeps
/// it from being used twice.
private struct EyeStreamHandle: @unchecked Sendable {
    let stream: SCStream
}
