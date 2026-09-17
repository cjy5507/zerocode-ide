import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class SoundSenseTests: XCTestCase {
    private func config(eventsMax: Double = 256) -> SoundSenseConfig {
        SoundSenseConfig(
            windowSeconds: 0.75, hopSeconds: 0.05, sampleRate: 16_000, minConfidence: 0.5,
            eventsMax: eventsMax, debounceMs: 500, idleStopMs: 600_000, ignoredLabels: ["silence"]
        )!
    }

    func testTheTableMustBeWholeAndSane() {
        XCTAssertNil(SoundSenseConfig(
            windowSeconds: 0.75, hopSeconds: nil, sampleRate: 16_000, minConfidence: 0.5,
            eventsMax: 256, debounceMs: 500, idleStopMs: 600_000, ignoredLabels: []
        ), "a missing number is refused, never guessed")
        XCTAssertNil(SoundSenseConfig(
            windowSeconds: 0.75, hopSeconds: 1, sampleRate: 16_000, minConfidence: 0.5,
            eventsMax: 256, debounceMs: 500, idleStopMs: 600_000, ignoredLabels: []
        ), "a hop longer than its window")
        XCTAssertEqual(config().overlapFactor, 1 - 0.05 / 0.75, accuracy: 1e-12)
    }

    func testASoundThatGoesOnIsOneEventAndAPauseMakesANewOne() {
        var ring = SoundRing(config: config())
        XCTAssertEqual(ring.note(label: "siren", confidence: 0.9, atMs: 0)?.seq, 1)
        for at in stride(from: Int64(50), through: 2_000, by: 50) {
            XCTAssertNil(ring.note(label: "siren", confidence: 0.9, atMs: at), "the same siren at \(at) ms")
        }
        XCTAssertEqual(ring.note(label: "siren", confidence: 0.9, atMs: 2_600)?.seq, 2, "heard again after a pause")
        XCTAssertEqual(ring.note(label: "knock", confidence: 0.7, atMs: 2_610)?.seq, 3, "another label is another sound")
        XCTAssertEqual(ring.latest, 3)
    }

    func testQuietAndUnsureAreNotEvents() {
        var ring = SoundRing(config: config())
        XCTAssertNil(ring.note(label: "silence", confidence: 0.99, atMs: 0))
        XCTAssertNil(ring.note(label: "bell", confidence: 0.49, atMs: 0))
        XCTAssertTrue(ring.events.isEmpty)
        XCTAssertEqual(ring.latest, 0)
    }

    func testTheRingKeepsTheNewestAndAReaderWaitsPastItsCursor() {
        var ring = SoundRing(config: config(eventsMax: 2))
        for (index, label) in ["a", "b", "c"].enumerated() {
            ring.note(label: label, confidence: 1, atMs: Int64(index))
        }
        XCTAssertEqual(ring.events.map(\.label), ["b", "c"], "the oldest leaves first")
        XCTAssertEqual(ring.events(after: 2).map(\.label), ["c"])
        XCTAssertTrue(ring.events(after: ring.latest).isEmpty)
    }
}
