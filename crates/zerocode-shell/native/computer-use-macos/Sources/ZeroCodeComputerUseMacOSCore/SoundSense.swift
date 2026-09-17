import Foundation

/// The ears' numbers, as the window's table sends them with `listenStart`
/// (`zerocode_core::computer_use::sound_table`) — the helper keeps none of
/// its own, so there is one table to change.
public struct SoundSenseConfig: Equatable, Sendable {
    public let windowSeconds: Double
    public let hopSeconds: Double
    public let sampleRate: Int
    public let minConfidence: Double
    public let eventsMax: Int
    public let debounceMs: Int64
    public let idleStopMs: Int64
    public let ignoredLabels: Set<String>

    /// Nil unless every number is there and makes sense.
    public init?(
        windowSeconds: Double?,
        hopSeconds: Double?,
        sampleRate: Double?,
        minConfidence: Double?,
        eventsMax: Double?,
        debounceMs: Double?,
        idleStopMs: Double?,
        ignoredLabels: [String]?
    ) {
        guard let windowSeconds, windowSeconds > 0,
              let hopSeconds, hopSeconds > 0, hopSeconds <= windowSeconds,
              let sampleRate, sampleRate >= 1,
              let minConfidence, (0...1).contains(minConfidence),
              let eventsMax, eventsMax >= 1,
              let debounceMs, debounceMs >= 0,
              let idleStopMs, idleStopMs > 0,
              let ignoredLabels
        else { return nil }
        self.windowSeconds = windowSeconds
        self.hopSeconds = hopSeconds
        self.sampleRate = Int(sampleRate)
        self.minConfidence = minConfidence
        self.eventsMax = Int(eventsMax)
        self.debounceMs = Int64(debounceMs)
        self.idleStopMs = Int64(idleStopMs)
        self.ignoredLabels = Set(ignoredLabels)
    }

    /// The classifier's overlap that starts one window every hop.
    public var overlapFactor: Double { 1 - hopSeconds / windowSeconds }
}

/// One sound the classifier named.
public struct SoundEvent: Equatable, Sendable {
    public let seq: Int
    public let label: String
    public let confidence: Double
    public let atMs: Int64

    public init(seq: Int, label: String, confidence: Double, atMs: Int64) {
        self.seq = seq
        self.label = label
        self.confidence = confidence
        self.atMs = atMs
    }
}

/// What the listener heard: a bounded ring of events numbered in the order
/// they were heard, one per sound. The classifier names a sound every hop
/// while it lasts; a label heard again before the debounce has passed since
/// it was last heard is the same sound going on, not a new one.
public struct SoundRing: Equatable, Sendable {
    public let config: SoundSenseConfig
    public private(set) var events: [SoundEvent] = []
    /// The number of the newest event — the cursor a reader waits past.
    public private(set) var latest = 0
    private var lastHeard: [String: Int64] = [:]

    public init(config: SoundSenseConfig) {
        self.config = config
    }

    /// Note one classification; the event it makes, if it is a new sound.
    @discardableResult
    public mutating func note(label: String, confidence: Double, atMs: Int64) -> SoundEvent? {
        guard confidence >= config.minConfidence, !config.ignoredLabels.contains(label) else { return nil }
        defer { lastHeard[label] = atMs }
        if let last = lastHeard[label], atMs - last < config.debounceMs {
            return nil
        }
        latest += 1
        let event = SoundEvent(seq: latest, label: label, confidence: confidence, atMs: atMs)
        events.append(event)
        if events.count > config.eventsMax {
            events.removeFirst(events.count - config.eventsMax)
        }
        return event
    }

    public func events(after seq: Int) -> [SoundEvent] {
        events.filter { $0.seq > seq }
    }
}
