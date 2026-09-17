import CoreVideo
import Darwin

/// Exact equality for the BGRA frames requested by ScreenEye. A repeated
/// invalidation can reuse the encoded picture without hiding real changes.
public enum EyePixels {
    /// The first frame establishes a baseline; calling it a full-display
    /// repaint would make a newly opened watch ignore the whole screen as
    /// existing background motion. Later differences are never age-filtered.
    public static func isRepaint(_ previous: CVPixelBuffer?, _ next: CVPixelBuffer) -> Bool {
        guard let previous else { return false }
        return !same(previous, next)
    }

    public static func same(_ previous: CVPixelBuffer?, _ next: CVPixelBuffer) -> Bool {
        guard let previous,
              CVPixelBufferGetPixelFormatType(previous) == kCVPixelFormatType_32BGRA,
              CVPixelBufferGetPixelFormatType(next) == kCVPixelFormatType_32BGRA,
              CVPixelBufferGetWidth(previous) == CVPixelBufferGetWidth(next),
              CVPixelBufferGetHeight(previous) == CVPixelBufferGetHeight(next),
              CVPixelBufferLockBaseAddress(previous, .readOnly) == kCVReturnSuccess
        else { return false }
        defer { CVPixelBufferUnlockBaseAddress(previous, .readOnly) }
        guard CVPixelBufferLockBaseAddress(next, .readOnly) == kCVReturnSuccess else { return false }
        defer { CVPixelBufferUnlockBaseAddress(next, .readOnly) }
        guard let lhs = CVPixelBufferGetBaseAddress(previous),
              let rhs = CVPixelBufferGetBaseAddress(next) else { return false }
        let count = CVPixelBufferGetWidth(next) * MemoryLayout<UInt32>.size
        let lhsStride = CVPixelBufferGetBytesPerRow(previous)
        let rhsStride = CVPixelBufferGetBytesPerRow(next)
        guard count <= lhsStride, count <= rhsStride else { return false }
        for row in 0..<CVPixelBufferGetHeight(next) {
            if memcmp(lhs.advanced(by: row * lhsStride), rhs.advanced(by: row * rhsStride), count) != 0 {
                return false
            }
        }
        return true
    }
}
