import CoreGraphics

/// A region capture at point resolution (t-10424): the probe asks for one
/// window's region every tick, and the whole display at its backing
/// resolution, cropped and scaled down, cost more than the region is worth.
/// Only a request that names `pointScale` is planned here; one that does not
/// keeps the whole-display capture, cropped, unchanged.
public enum DesktopPointCapture {
    /// The largest pixels-per-point a caller may ask for: the finest display
    /// backing scale there is (3 on the densest phones' mirrors), with room.
    public static let maxPointScale: Double = 4

    public enum Plan: Equatable {
        /// `pointScale` absent: the path as it was.
        case wholeDisplayThenCrop
        /// `sourceRect` in global points is captured and drawn at
        /// `pixelWidth` × `pixelHeight`, `region × pointScale`, rounded.
        case region(sourceRect: CGRect, pixelWidth: Int, pixelHeight: Int)
    }

    public static func pointScale(_ value: Double?) -> Result<Double?, ActionArgumentValidationError> {
        guard let value else { return .success(nil) }
        guard value.isFinite, value > 0, value <= maxPointScale else {
            return .failure(ActionArgumentValidationError("pointScale must be a number above 0 and at most \(Int(maxPointScale))"))
        }
        return .success(value)
    }

    /// nil when the region does not touch the display.
    public static func plan(region: CGRect?, displayBounds: CGRect, pointScale: Double?) -> Plan? {
        guard let pointScale else { return .wholeDisplayThenCrop }
        let visible = region.map { $0.intersection(displayBounds) } ?? displayBounds
        guard !visible.isNull, visible.width > 0, visible.height > 0 else { return nil }
        let scale = CGFloat(pointScale)
        return .region(
            sourceRect: visible,
            pixelWidth: max(1, Int((visible.width * scale).rounded())),
            pixelHeight: max(1, Int((visible.height * scale).rounded()))
        )
    }
}
