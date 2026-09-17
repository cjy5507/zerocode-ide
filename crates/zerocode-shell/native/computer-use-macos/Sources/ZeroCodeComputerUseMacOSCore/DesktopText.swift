import CoreGraphics
import Foundation
import Vision

/// A line of text read off the pixels, placed on the screen in points.
public struct RecognizedLine: Equatable {
    public let text: String
    public let confidence: Double
    public let frame: CGRect

    public init(text: String, confidence: Double, frame: CGRect) {
        self.text = text
        self.confidence = confidence
        self.frame = frame
    }
}

/// Where a Vision box lands on the screen: Vision measures from the bottom-left
/// of the image in fractions; the screen counts points from the top-left of
/// the captured area.
public func recognizedTextRect(box: CGRect, origin: CGPoint, pointsWidth: CGFloat, pointsHeight: CGFloat) -> CGRect {
    CGRect(
        x: origin.x + box.minX * pointsWidth,
        y: origin.y + (1 - box.maxY) * pointsHeight,
        width: box.width * pointsWidth,
        height: box.height * pointsHeight
    )
}

/// The languages a person's screen may show, offered to the recogniser in
/// this order when the platform knows them.
public let recognitionLanguagePreference = ["ko-KR", "en-US", "ja-JP", "zh-Hans", "es-ES"]

/// Read every line of text in an image, top to bottom then left to right,
/// each placed on the screen. Nothing here needs a permission: the caller
/// already holds the pixels.
public func recognizeText(in image: CGImage, origin: CGPoint, pointsWidth: CGFloat, pointsHeight: CGFloat) throws -> [RecognizedLine] {
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    request.automaticallyDetectsLanguage = true
    if let supported = try? request.supportedRecognitionLanguages() {
        let wanted = recognitionLanguagePreference.filter { supported.contains($0) }
        if !wanted.isEmpty { request.recognitionLanguages = wanted }
    }
    let handler = VNImageRequestHandler(cgImage: image, options: [:])
    try handler.perform([request])
    let lines = (request.results ?? []).compactMap { observation -> RecognizedLine? in
        guard let candidate = observation.topCandidates(1).first else { return nil }
        return RecognizedLine(
            text: candidate.string,
            confidence: Double(candidate.confidence),
            frame: recognizedTextRect(box: observation.boundingBox, origin: origin, pointsWidth: pointsWidth, pointsHeight: pointsHeight)
        )
    }
    return readingOrder(lines)
}

/// Lines as a person reads them: top to bottom, and left to right within
/// a line's height.
public func readingOrder(_ lines: [RecognizedLine]) -> [RecognizedLine] {
    lines.sorted { lhs, rhs in
        if abs(lhs.frame.midY - rhs.frame.midY) > min(lhs.frame.height, rhs.frame.height) / 2 {
            return lhs.frame.midY < rhs.frame.midY
        }
        return lhs.frame.minX < rhs.frame.minX
    }
}

/// What a `find` looks for in an accessibility element: a role, a label, a
/// piece of text — each compared the way a person reads them.
public struct ElementQuery {
    public let text: String?
    public let role: String?
    public let label: String?

    public init(text: String?, role: String?, label: String?) {
        self.text = text?.trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty
        self.role = role?.trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty
        self.label = label?.trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty
    }

    public var isEmpty: Bool { text == nil && role == nil && label == nil }

    public func matches(role elementRole: String?, label elementLabel: String?, value: String?, description: String?) -> Bool {
        if let role, normalizedRole(role) != normalizedRole(elementRole ?? "") { return false }
        if let label {
            let named = [elementLabel, description].compactMap { $0 }
            guard named.contains(where: { $0.localizedCaseInsensitiveContains(label) }) else { return false }
        }
        if let text {
            let shown = [elementLabel, value, description].compactMap { $0 }
            guard shown.contains(where: { $0.localizedCaseInsensitiveContains(text) }) else { return false }
        }
        return true
    }

    /// "button", "Button" and "AXButton" name the same role.
    func normalizedRole(_ role: String) -> String {
        let lowered = role.lowercased()
        return lowered.hasPrefix("ax") ? String(lowered.dropFirst(2)) : lowered
    }

    /// The query as it was asked, for an answer that names it.
    public var words: String {
        [text.map { "--text «\($0)»" }, role.map { "--role \($0)" }, label.map { "--label «\($0)»" }]
            .compactMap { $0 }
            .joined(separator: " ")
    }

    /// The one control a click by reading presses (§2.5). Among the faces the
    /// query matches the way a person reads, one that reads *exactly* the
    /// asked words wins — "Save" over "Save As…" — else the match must be
    /// alone. None, or several that tie, is answered as such: the press
    /// never guesses between two controls.
    public func choose(among faces: [ElementFace]) -> ElementChoice {
        let matched = faces.filter { matches(role: $0.role, label: $0.label, value: $0.value, description: $0.description) }
        switch matched.count {
        case 0: return .none
        case 1: return .one(matched[0].index)
        default: break
        }
        let exact = matched.filter { readsExactly($0) }
        if exact.count == 1 { return .one(exact[0].index) }
        return .many(exact.isEmpty ? matched : exact)
    }

    /// Whether the face reads the asked words whole: the text as its label,
    /// value or description; the label as its label or description.
    func readsExactly(_ face: ElementFace) -> Bool {
        let same: (String?, String) -> Bool = { shown, asked in
            shown?.trimmingCharacters(in: .whitespacesAndNewlines).caseInsensitiveCompare(asked) == .orderedSame
        }
        if let text, ![face.label, face.value, face.description].contains(where: { same($0, text) }) { return false }
        if let label, ![face.label, face.description].contains(where: { same($0, label) }) { return false }
        return text != nil || label != nil
    }
}

/// What one control reads, as `find` answers it: enough to choose by.
public struct ElementFace: Equatable {
    public let index: Int
    public let role: String?
    public let label: String?
    public let value: String?
    public let description: String?

    public init(index: Int, role: String?, label: String?, value: String?, description: String?) {
        self.index = index
        self.role = role
        self.label = label
        self.value = value
        self.description = description
    }

    /// One line an answer names a candidate by: `12 button «Save»`.
    public var said: String {
        let reads = [label, value, description].compactMap { $0 }.first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        return "\(index) \(role ?? "?")\(reads.map { " «\($0)»" } ?? "")"
    }
}

/// A click by reading's choice: the one control, none, or the several that tie.
public enum ElementChoice: Equatable {
    case one(Int)
    case none
    case many([ElementFace])
}

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
