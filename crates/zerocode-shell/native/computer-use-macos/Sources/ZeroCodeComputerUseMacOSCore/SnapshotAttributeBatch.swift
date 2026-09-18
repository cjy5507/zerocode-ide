import ApplicationServices
import Foundation

/// Every Accessibility attribute the snapshot walk reads off one node, named
/// once, so a node costs one cross-process round trip instead of one per
/// attribute.
///
/// Each `AXUIElementCopyAttributeValue` is a synchronous call into the app
/// being observed, and the walk asked fourteen of them per node — a 528-node
/// Teams window paid roughly ten thousand round trips, 0.64 ms a node, and an
/// app that answers slowly (KakaoTalk: 57 elements, 1,674 ms) blocked us for
/// exactly as long as it took. `AXUIElementCopyMultipleAttributeValues` asks
/// for the whole list in one call.
///
/// The Windows provider already reads this way and says so in
/// `computer_use/windows/uia.rs`: a cache request names every property the
/// renderer will ask about, and anything not listed costs a live round trip
/// later. This table is that request. It is a declaration about *speed*, not
/// about meaning: an attribute missing from it still answers, through the
/// single read the reader falls back to, so the table can never change what a
/// snapshot says — only what it costs. `SnapshotAttributeBatchSourceTests`
/// keeps it level with the attributes the walk actually names.
public enum SnapshotNodeAttributes {
    /// The attributes one node is read for, in the order they go on the wire.
    public static let all: [String] = [
        kAXRoleAttribute as String,
        kAXSubroleAttribute as String,
        kAXRoleDescriptionAttribute as String,
        kAXTitleAttribute as String,
        kAXDescriptionAttribute as String,
        kAXValueAttribute as String,
        kAXURLAttribute as String,
        kAXPlaceholderValueAttribute as String,
        // No `kAX…` constant names this one; the fields that answer it
        // instead of AXPlaceholderValue are read the same way.
        "AXPlaceholder",
        kAXPositionAttribute as String,
        kAXSizeAttribute as String,
        kAXSelectedAttribute as String,
        kAXExpandedAttribute as String,
        kAXEnabledAttribute as String,
        kAXChildrenAttribute as String,
        kAXRowsAttribute as String,
    ]
}

/// What one multi-attribute read came back with, judged attribute by
/// attribute the way the single reads it replaces judged their own result.
///
/// `AXUIElementCopyMultipleAttributeValues` reports a partial failure as a
/// value: the entry for an attribute the element does not answer is an
/// `AXValue` carrying an `AXError` (and some elements answer with `kCFNull`).
/// A single `AXUIElementCopyAttributeValue` returned nothing at all in that
/// case, and every caller upstream reads the result as an optional. So an
/// error entry and a null entry both have to come back here as "absent", or
/// one unsupported attribute would otherwise be rendered as a value and one
/// node's line would change.
public enum SnapshotAttributeBatch {
    /// The read for one attribute, or `nil` where a single read would have
    /// returned nothing.
    ///
    /// Returns `nil` for the whole batch when the answer does not line up
    /// with the request — the caller then falls back to single reads rather
    /// than guessing which attribute each value belongs to.
    public static func decode(attributes: [String], values: [CFTypeRef]) -> [String: CFTypeRef?]? {
        guard attributes.count == values.count else { return nil }
        var decoded: [String: CFTypeRef?] = [:]
        decoded.reserveCapacity(attributes.count)
        for (attribute, value) in zip(attributes, values) {
            // `updateValue`, not the subscript: an absent attribute is a key
            // that answers nothing, not a key the batch never mentioned.
            decoded.updateValue(carriesValue(value) ? value : nil, forKey: attribute)
        }
        return decoded
    }

    /// Whether this entry is a value the element answered with, rather than
    /// the read having failed.
    public static func carriesValue(_ value: CFTypeRef) -> Bool {
        let typeID = CFGetTypeID(value)
        if typeID == CFNullGetTypeID() {
            return false
        }
        if typeID == AXValueGetTypeID(), AXValueGetType(value as! AXValue) == .axError {
            return false
        }
        return true
    }
}
