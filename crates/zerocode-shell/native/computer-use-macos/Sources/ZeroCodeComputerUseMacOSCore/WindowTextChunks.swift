public struct WindowTextChunks: Sequence {
    public static let maxUTF16Count = 20
    private let text: String

    public init(_ text: String) {
        self.text = text
    }

    public func makeIterator() -> Iterator {
        Iterator(scalars: text.unicodeScalars.makeIterator())
    }

    public struct Iterator: IteratorProtocol {
        fileprivate var scalars: String.UnicodeScalarView.Iterator
        private var pending: Unicode.Scalar?

        fileprivate init(scalars: String.UnicodeScalarView.Iterator) {
            self.scalars = scalars
        }

        public mutating func next() -> String? {
            var chunk = String()
            chunk.reserveCapacity(WindowTextChunks.maxUTF16Count)
            var units = 0
            while let scalar = pending ?? scalars.next() {
                pending = nil
                let control = scalar.value < 0x20 || scalar.value == 0x7F
                if control && !chunk.isEmpty {
                    pending = scalar
                    break
                }
                let count = scalar.value > 0xFFFF ? 2 : 1
                guard units + count <= WindowTextChunks.maxUTF16Count else {
                    pending = scalar
                    break
                }
                chunk.unicodeScalars.append(scalar)
                units += count
                if control { break }
            }
            return chunk.isEmpty ? nil : chunk
        }
    }
}
