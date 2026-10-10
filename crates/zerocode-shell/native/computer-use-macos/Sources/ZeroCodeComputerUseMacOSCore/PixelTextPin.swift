import CoreGraphics
import Foundation

public struct PixelTextPin {
    public let text: String
    public let frame: CGRect
    public let window: CGRect
    public let tolerance: Double
    public let minimumConfidence: Double

    public init(text: String, frame: CGRect, window: CGRect, tolerance: Double, minimumConfidence: Double) {
        self.text = text
        self.frame = frame
        self.window = window
        self.tolerance = tolerance
        self.minimumConfidence = minimumConfidence
    }

    public func windowStands(_ current: CGRect) -> Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && tolerance.isFinite && tolerance >= 0
            && minimumConfidence.isFinite && (0...1).contains(minimumConfidence)
            && valid(frame) && valid(window) && valid(current)
            && window.contains(frame) && agrees(window, current)
    }

    public func holds(lines: [RecognizedLine], window current: CGRect) -> Bool {
        guard windowStands(current) else { return false }
        var count = 0
        for line in lines where line.text == text && line.confidence.isFinite
            && line.confidence >= minimumConfidence && line.confidence <= 1
            && valid(line.frame) && agrees(frame, line.frame) {
            count += 1
            if count > 1 { return false }
        }
        return count == 1
    }

    private func agrees(_ before: CGRect, _ after: CGRect) -> Bool {
        abs(before.minX - after.minX) <= tolerance
            && abs(before.minY - after.minY) <= tolerance
            && abs(before.width - after.width) <= tolerance
            && abs(before.height - after.height) <= tolerance
    }

    private func valid(_ rect: CGRect) -> Bool {
        [rect.origin.x, rect.origin.y, rect.width, rect.height].allSatisfy { $0.isFinite }
            && rect.size.width > 0 && rect.size.height > 0
    }
}
