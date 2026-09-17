import AVFoundation
import CoreMedia
import Foundation
import ScreenCaptureKit
import SoundAnalysis
import ZeroCodeComputerUseMacOSCore

/// The ears (docs/design/computer-use-full-operator.md §7.1): the machine's
/// sound — or one app's — through ScreenCaptureKit, which the Screen & System
/// Audio Recording row the eyes already hold covers, named by the system's
/// built-in sound classifier into a `SoundRing` the window reads. Every
/// number comes from the window's table with `listenStart`.
final class SoundListener: NSObject, SCStreamOutput, SCStreamDelegate, SNResultsObserving, @unchecked Sendable {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var current: SoundListener?
    /// ScreenCaptureKit captures a picture beside the sound whether or not it
    /// is wanted: the smallest it accepts, as seldom as it allows.
    private static let idlePictureSide = 2
    private static let idlePictureInterval = CMTime(value: 1, timescale: 1)
    /// How long starting or stopping the stream may take before it is refused.
    private static let streamTimeoutSeconds: TimeInterval = 5

    private let config: SoundSenseConfig
    private let source: String
    private let queue = DispatchQueue(label: "dev.zerocode.computer-use.sound")
    /// The idle timer's own queue: closing the ears ends with a `queue.sync`,
    /// which on `queue` itself is libdispatch's deadlock trap.
    private static let timers = DispatchQueue(label: "dev.zerocode.computer-use.sound.idle")
    private let stateLock = NSLock()
    private var ring: SoundRing
    private var stream: SCStream?
    private var failure: String?
    private var readGeneration = 0
    private var heardBuffers = 0
    private let startedAtMs = SoundListener.nowMs()
    // Touched only on `queue`, where the stream delivers its sound.
    private var analyzer: SNAudioStreamAnalyzer?
    private var framePosition: AVAudioFramePosition = 0

    private init(config: SoundSenseConfig, source: String) {
        self.config = config
        self.source = source
        self.ring = SoundRing(config: config)
    }

    // MARK: the three requests

    /// Start listening — the whole machine, or one app — replacing any
    /// listener already running.
    static func start(config: SoundSenseConfig, app: AppDescriptor?) throws -> [String: Any] {
        guard screenCaptureTrusted() else {
            throw ProviderError.coded(
                "permission_denied",
                "the ears use the Screen & System Audio Recording permission the screenshots use (zerocode-computer permissions --id screenshots)"
            )
        }
        stop()
        let listener = SoundListener(config: config, source: app?.name ?? "system")
        try listener.begin(pid: app?.pid)
        lock.lock()
        current = listener
        lock.unlock()
        listener.touch()
        return listener.state(after: nil)
    }

    @discardableResult
    static func stop() -> [String: Any] {
        lock.lock()
        let listener = current
        current = nil
        lock.unlock()
        guard let listener else { return ["listening": false] }
        listener.end()
        var state = listener.state(after: nil)
        state["listening"] = false
        return state
    }

    /// What was heard after event `after`, and where the listener stands.
    static func read(after: Int) throws -> [String: Any] {
        lock.lock()
        let listener = current
        lock.unlock()
        guard let listener else {
            throw ProviderError.coded("not_listening", "nothing is listening — zerocode-computer listen-start first")
        }
        listener.touch()
        return listener.state(after: after)
    }

    // MARK: the stream

    private func begin(pid: pid_t?) throws {
        let config = self.config
        let stream = try BlockingAsync.run(timeout: Self.streamTimeoutSeconds, what: "starting to listen") { [self] in
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
            guard let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() }) ?? content.displays.first else {
                throw ProviderError.coded("unsupported_capability", "there is no display to listen through")
            }
            let filter: SCContentFilter
            if let pid {
                guard let app = content.applications.first(where: { $0.processID == pid }) else {
                    throw ProviderError.coded("app_not_found", "that app is not one ScreenCaptureKit can hear")
                }
                filter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
            } else {
                filter = SCContentFilter(display: display, excludingApplications: [], exceptingWindows: [])
            }
            let configuration = SCStreamConfiguration()
            configuration.capturesAudio = true
            configuration.excludesCurrentProcessAudio = true
            configuration.sampleRate = config.sampleRate
            configuration.channelCount = 1
            configuration.width = Self.idlePictureSide
            configuration.height = Self.idlePictureSide
            configuration.minimumFrameInterval = Self.idlePictureInterval
            let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
            try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
            try await stream.startCapture()
            return stream
        }
        stateLock.lock()
        self.stream = stream
        stateLock.unlock()
    }

    private func end() {
        stateLock.lock()
        let stream = self.stream
        self.stream = nil
        stateLock.unlock()
        if let stream {
            let handle = StreamHandle(stream: stream)
            _ = try? BlockingAsync.run(timeout: Self.streamTimeoutSeconds, what: "stopping the ears") {
                try await handle.stream.stopCapture()
            }
        }
        queue.sync {
            analyzer?.removeAllRequests()
            analyzer = nil
        }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, sampleBuffer.isValid, let buffer = Self.pcmBuffer(sampleBuffer) else { return }
        if analyzer == nil {
            let analyzer = SNAudioStreamAnalyzer(format: buffer.format)
            do {
                let request = try SNClassifySoundRequest(classifierIdentifier: .version1)
                request.windowDuration = CMTime(
                    seconds: config.windowSeconds,
                    preferredTimescale: CMTimeScale(config.sampleRate)
                )
                request.overlapFactor = config.overlapFactor
                try analyzer.add(request, withObserver: self)
            } catch {
                note(failure: "the sound classifier would not start: \(error.localizedDescription)")
                return
            }
            self.analyzer = analyzer
        }
        analyzer?.analyze(buffer, atAudioFramePosition: framePosition)
        framePosition += AVAudioFramePosition(buffer.frameLength)
        stateLock.lock()
        heardBuffers += 1
        stateLock.unlock()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        note(failure: "the sound stream stopped: \(error.localizedDescription)")
    }

    // MARK: the classifier

    func request(_ request: SNRequest, didProduce result: SNResult) {
        guard let result = result as? SNClassificationResult else { return }
        let atMs = Self.nowMs()
        stateLock.lock()
        for classification in result.classifications where classification.confidence >= config.minConfidence {
            ring.note(label: classification.identifier, confidence: classification.confidence, atMs: atMs)
        }
        stateLock.unlock()
    }

    func request(_ request: SNRequest, didFailWithError error: Error) {
        note(failure: "the sound classifier failed: \(error.localizedDescription)")
    }

    // MARK: state

    private func note(failure: String) {
        stateLock.lock()
        self.failure = failure
        stateLock.unlock()
    }

    /// A read keeps the ears open; nobody reading for the table's idle time
    /// closes them — sound is not captured for a listener nobody asks.
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

    private static func stop(ifCurrent listener: SoundListener) {
        lock.lock()
        let same = current === listener
        if same { current = nil }
        lock.unlock()
        if same { listener.end() }
    }

    private func state(after: Int?) -> [String: Any] {
        stateLock.lock()
        defer { stateLock.unlock() }
        var state: [String: Any] = [
            "listening": stream != nil && failure == nil,
            "source": source,
            "latest": ring.latest,
            "heard": heardBuffers,
            "since": startedAtMs,
            "windowSeconds": config.windowSeconds,
        ]
        if let failure { state["failure"] = failure }
        if let after {
            state["events"] = ring.events(after: after).map { event in
                ["seq": event.seq, "label": event.label, "confidence": event.confidence, "at": event.atMs] as [String: Any]
            }
        }
        return state
    }

    private static func nowMs() -> Int64 {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }

    /// One buffer of the stream's sound, as the classifier takes it.
    private static func pcmBuffer(_ sampleBuffer: CMSampleBuffer) -> AVAudioPCMBuffer? {
        guard let description = CMSampleBufferGetFormatDescription(sampleBuffer),
              let basic = CMAudioFormatDescriptionGetStreamBasicDescription(description),
              let format = AVAudioFormat(streamDescription: basic)
        else { return nil }
        let frames = AVAudioFrameCount(CMSampleBufferGetNumSamples(sampleBuffer))
        guard frames > 0, let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames) else { return nil }
        buffer.frameLength = frames
        let copied = CMSampleBufferCopyPCMDataIntoAudioBufferList(
            sampleBuffer,
            at: 0,
            frameCount: Int32(frames),
            into: buffer.mutableAudioBufferList
        )
        return copied == noErr ? buffer : nil
    }
}

/// A stream handed to the task that stops it; the listener's lock is what
/// keeps it from being used twice.
private struct StreamHandle: @unchecked Sendable {
    let stream: SCStream
}
