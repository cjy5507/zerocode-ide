import CoreGraphics

/// What a hit-test answered at an element's own centre.
enum AccessibilityCentreHit: Equatable {
    /// The element itself is on top there.
    case itself
    /// Something inside the element or around it — the centre is still its
    /// own to press (the desktop's rule, `AccessibilityCentre.answers`).
    case near
    /// Something else is on top there, or nothing answered.
    case elsewhere

    /// The `hit_at_centre` the tree carries: whether the centre is the
    /// element's own to press.
    var isItsOwn: Bool { self != .elsewhere }
}

/// Which walked elements stop the grid from asking about the points inside
/// them.
///
/// The grid exists for what the walk cannot reach: content a group with no
/// walked children hides and only a hit-test shows (a nav bar's, a tab bar's,
/// a scroll view's). So an element whose children were walked never blocks
/// it, and one with none blocks when it is too small to be such a group —
/// or, however wide, when the hit-test at its own centre answered the element
/// itself (t-6385): a row as wide as the screen that is on top at its own
/// centre is that row, not a group over rows it hides. Not past half the
/// screen, though: that is a canvas or a page — a map, a picture — and
/// controls float over the rest of it, found only by the grid. Maps' place
/// card lost six of its controls to a map view that answered its own centre.
///
/// Measured on an iPhone 17 simulator (iOS 26.5, Settings, t-6350): every
/// Settings row is 370 points wide, so each was taken for a container and
/// the grid asked 343 of its 364 points, finding only the status bar and the
/// toolbar's search field and dictation button. With such rows blocking, it
/// asked 116 and found the same seven — a look 580 → 323 ms — and the
/// General screen's requests fell 866 → 540 with nothing lost.
enum AccessibilityLeaf {
    /// The side past which an element that did not answer its own centre is
    /// taken for a container.
    static let containerSide: CGFloat = 250
    /// The share of the screen past which an element is a canvas or a page,
    /// whatever its centre answered (the core's `MARK_MAX_WINDOW_SHARE`, a
    /// source contract holds it).
    static let canvasShare: CGFloat = 0.5

    static func blocksGrid(
        walkedChildren: Int,
        frame: CGRect,
        centre: AccessibilityCentreHit?,
        screen: CGRect
    ) -> Bool {
        guard walkedChildren == 0 else { return false }
        if max(frame.width, frame.height) < containerSide { return true }
        return centre == .itself
            && frame.width * frame.height <= canvasShare * screen.width * screen.height
    }
}
