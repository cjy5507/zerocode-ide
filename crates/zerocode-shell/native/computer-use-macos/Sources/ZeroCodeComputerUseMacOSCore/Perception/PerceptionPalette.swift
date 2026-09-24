import Foundation

/// A borrowed BGRA plane: valid only while the caller keeps its buffer locked, never retained.
struct PerceptionPlane {
    let base: UnsafeRawPointer
    let width: Int
    let height: Int
    let bytesPerRow: Int

    init(base: UnsafeRawPointer, width: Int, height: Int, bytesPerRow: Int) {
        self.base = base
        self.width = width
        self.height = height
        self.bytesPerRow = bytesPerRow
    }

    init(_ pixels: ReflexPixels) {
        self.init(base: pixels.base, width: pixels.width, height: pixels.height, bytesPerRow: pixels.bytesPerRow)
    }

    /// The pixel at a position the caller has already placed inside the plane, as its bytes
    /// B, G, R, A read little-endian.
    @inline(__always)
    func pixel(_ x: Int, _ y: Int) -> UInt32 {
        base.loadUnaligned(fromByteOffset: y &* bytesPerRow &+ x &<< 2, as: UInt32.self)
    }
}

/// A palette compiled for the inner loop. For each channel value there is a mask of the
/// classes (bit k − 1 for class k) and the ground (the top bit) that accept it, so a sample is
/// three lookups and two ANDs. Classes and ground never overlap (the spec's checks), so at most
/// one bit survives.
struct PerceptionPalette {
    /// What a sample is when it is neither a class nor the ground.
    static let foreign = 0
    /// What a sample is when it is the ground.
    static let ground = -1
    private static let groundBit: UInt16 = 0x8000

    /// 768 masks: red values, then green, then blue.
    private let masks: [UInt16]
    let classes: Int
    let hasGround: Bool

    /// Nil when the palette has more classes than a mask can name besides the ground.
    init?(_ spec: PerceptionColorSpec) {
        guard spec.classes.count < Self.groundBit.trailingZeroBitCount else { return nil }
        var masks = [UInt16](repeating: 0, count: 768)
        func accept(_ item: PerceptionColorClass, bit: UInt16) {
            for (channel, value) in [item.r, item.g, item.b].enumerated() {
                let low = max(0, Int(value) - Int(item.tolerance))
                let high = min(255, Int(value) + Int(item.tolerance))
                for level in low...high { masks[channel * 256 + level] |= bit }
            }
        }
        for (at, item) in spec.classes.enumerated() { accept(item, bit: 1 << UInt16(at)) }
        if let ground = spec.ground { accept(ground, bit: Self.groundBit) }
        self.masks = masks
        classes = spec.classes.count
        hasGround = spec.ground != nil
    }

    /// Runs `body` with the masks pinned for `classify`.
    func withMasks<T>(_ body: (UnsafePointer<UInt16>) throws -> T) rethrows -> T {
        try masks.withUnsafeBufferPointer { try body($0.baseAddress!) }
    }

    /// The class a pixel is (1-based), `ground`, or `foreign`.
    @inline(__always)
    static func classify(_ pixel: UInt32, _ masks: UnsafePointer<UInt16>) -> Int {
        let hit = masks[Int((pixel >> 16) & 0xFF)] & masks[256 + Int((pixel >> 8) & 0xFF)] & masks[512 + Int(pixel & 0xFF)]
        if hit == 0 { return foreign }
        if hit & groundBit != 0 { return ground }
        return hit.trailingZeroBitCount + 1
    }
}
