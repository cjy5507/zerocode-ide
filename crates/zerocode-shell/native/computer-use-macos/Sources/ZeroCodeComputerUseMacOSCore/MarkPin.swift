import CoreGraphics
import Foundation

/// The pin a click by a mark's number carries (the core's
/// `computer_use_protocol::marks::Pin`, mirrored case for case — the one rule
/// of the marks the helper holds): the element at the index is still the one
/// the mark was drawn on. Same identity, same words, same place within the
/// tolerance the request carries; the helper keeps no numbers of its own.
public enum MarkPin {
    /// Where typing goes (the core's `TEXT_ENTRY_ROLES`, a source contract
    /// holds them): a plain click on one focuses it, else presses it — never
    /// its confirm action, which is a Return.
    public static let textEntryRoles: Set<String> = ["AXTextField", "AXSearchField", "AXTextArea"]
    /// The containers that clip what is inside them (the core's `MARK_CLIP_ROLES`).
    public static let clipRoles: Set<String> = ["AXScrollArea"]
    /// How many parents a hit-test's answer, or the marked control, is walked
    /// to meet the other (the core's `MARK_HIT_DEPTH`).
    public static let hitDepth = 32

    /// A frame cut by a clip; a frame cut off whole keeps its corner and no
    /// size (the core's `render::clipped`).
    public static func clipped(_ frame: CGRect, by clip: CGRect?) -> CGRect {
        guard let clip else { return frame }
        let cut = frame.intersection(clip)
        return cut.isNull ? CGRect(origin: frame.origin, size: .zero) : cut
    }

    public static func holds(
        expectedSignature: String,
        expectedName: String,
        expectedContext: String,
        expectedFrame: CGRect,
        actualSignature: String?,
        actualName: String?,
        actualContext: String?,
        actualFrame: CGRect?,
        tolerance: CGFloat
    ) -> Bool {
        guard actualSignature == expectedSignature,
              (actualName ?? "") == expectedName,
              (actualContext ?? "") == expectedContext,
              let actualFrame
        else {
            return false
        }
        return abs(actualFrame.origin.x - expectedFrame.origin.x) <= tolerance &&
            abs(actualFrame.origin.y - expectedFrame.origin.y) <= tolerance &&
            abs(actualFrame.width - expectedFrame.width) <= tolerance &&
            abs(actualFrame.height - expectedFrame.height) <= tolerance
    }

    /// A request's pin, read whole or refused: the four keys together, every
    /// number real, the size and the tolerance not negative. `nil` when the
    /// request carries none.
    public struct Parsed: Equatable {
        public let signature: String
        public let name: String
        public let context: String
        public let frame: CGRect
        public let tolerance: CGFloat
    }

    public enum ParseError: Error, Equatable {
        case incomplete
        case invalid(String)
    }

    /// The words the window, the helper and the Windows provider share
    /// (the core's `marks` keys; a source contract holds them).
    public static let elementFramesKey = "elementFrames"
    public static let everyLayerKey = "everyLayer"
    public static let signatureKey = "elementSignature"
    public static let nameKey = "elementName"
    public static let contextKey = "elementContext"
    public static let frameKey = "elementFrame"
    public static let toleranceKey = "markTolerance"

    public static func parse(
        signature: String?,
        name: String?,
        context: String?,
        frame: [String: Double]?,
        tolerance: Double?,
        present: Int
    ) throws -> Parsed? {
        if present == 0 { return nil }
        guard present == 5, let signature, let name, let context, let frame, let tolerance else {
            throw ParseError.incomplete
        }
        let keys = ["x", "y", "width", "height"]
        let numbers = keys.compactMap { frame[$0] }
        guard numbers.count == keys.count, numbers.allSatisfy(\.isFinite) else {
            throw ParseError.invalid("\(frameKey) needs finite x, y, width and height")
        }
        guard numbers[2] >= 0, numbers[3] >= 0 else {
            throw ParseError.invalid("\(frameKey) needs a non-negative size")
        }
        guard tolerance.isFinite, tolerance >= 0 else {
            throw ParseError.invalid("\(toleranceKey) must be a non-negative number")
        }
        return Parsed(
            signature: signature,
            name: name,
            context: context,
            frame: CGRect(x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3]),
            tolerance: CGFloat(tolerance)
        )
    }
}
