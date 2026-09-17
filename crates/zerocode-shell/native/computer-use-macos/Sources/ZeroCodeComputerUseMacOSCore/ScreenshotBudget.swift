import CoreGraphics

/// How large a screenshot may be, and the scales it is encoded at until it
/// fits — the table `zerocode_core::computer_use_protocol` holds for every
/// provider (`MAX_SCREENSHOT_PNG_BYTES`, `SCREENSHOT_RESIZE_*`,
/// `screenshot_resize_ladder`), kept here value for value — a source contract
/// in the shell crate reads both and refuses a drift.
public enum ScreenshotBudget {
    /// A PNG larger than this walks down the ladder until it fits.
    public static let maxPngBytes = 900_000
    /// The first rung brings the long edge to this many pixels.
    public static let startLongEdge: CGFloat = 1280
    /// Each further rung multiplies the scale by this.
    public static let step: CGFloat = 0.85
    /// No further rung brings the long edge below this many pixels.
    public static let floorLongEdge: CGFloat = 320

    /// The scales a picture is encoded at, in order: the first brings the long
    /// edge to the budget's (1 is the picture as captured) and is always tried,
    /// so a full-resolution encode of a Retina frame is never paid for; further
    /// rungs step down while the long edge stays at or above the floor.
    public static func ladder(width: Int, height: Int) -> [CGFloat] {
        let longEdge = max(CGFloat(max(width, height)), 1)
        var rungs = [min(1, startLongEdge / longEdge)]
        var next = rungs[0] * step
        while next * longEdge >= floorLongEdge {
            rungs.append(next)
            next *= step
        }
        return rungs
    }
}
