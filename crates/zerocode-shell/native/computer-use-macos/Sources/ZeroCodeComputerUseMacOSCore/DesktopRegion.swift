import CoreGraphics

/// A region in screen points, on a display whose capture is `pixelWidth` wide,
/// as the pixel rectangle to crop out of that capture — or nil when the region
/// does not touch the display at all. Points are the desktop's (Quartz, origin
/// at the main display's top-left); pixels are the display's own, so the scale
/// is the capture's width over the display's width in points.
public func desktopRegionInPixels(region: CGRect, displayBounds: CGRect, pixelWidth: Int) -> CGRect? {
    guard displayBounds.width > 0, displayBounds.height > 0, pixelWidth > 0 else { return nil }
    let visible = region.intersection(displayBounds)
    guard !visible.isNull, visible.width > 0, visible.height > 0 else { return nil }
    let scale = CGFloat(pixelWidth) / displayBounds.width
    return CGRect(
        x: (visible.minX - displayBounds.minX) * scale,
        y: (visible.minY - displayBounds.minY) * scale,
        width: visible.width * scale,
        height: visible.height * scale
    ).integral
}

/// Which of `displays` — the main one first, as `--display N` counts them —
/// a capture of `region` is taken on: the one that shows the region's centre
/// (a box about a point near an edge belongs to the point's display, not to
/// the first one it grazes), else the first it touches, else the main one.
/// Points are the desktop's, so a display left of or above the main one has
/// negative ones (t-66753 C2).
public func desktopDisplayIndex(showing region: CGRect?, among displays: [CGRect]) -> Int {
    guard let region else { return 0 }
    let centre = CGPoint(x: region.midX, y: region.midY)
    return displays.firstIndex { $0.contains(centre) }
        ?? displays.firstIndex { $0.intersects(region) }
        ?? 0
}

/// The bytes-per-point scale a screenshot reports: how many pixels one point
/// of the captured area holds, so a coordinate read off the image goes back
/// to the desktop by dividing.
public func desktopScreenshotScale(pixelWidth: Int, pointsWidth: CGFloat) -> Double {
    Double(pixelWidth) / Double(max(pointsWidth, 1))
}
