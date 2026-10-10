import Foundation

public struct BoundedLineReader {
    public static let maxRequestBytes = 64 * 1024 * 1024

    public enum Failure: Error, Equatable {
        case tooLarge
        case invalidRead
    }

    private let limit: Int
    private var buffer = [UInt8](repeating: 0, count: 8192)
    private var consumed = 0
    private var available = 0
    private var finished = false

    public init(limit: Int = Self.maxRequestBytes) {
        self.limit = max(0, limit)
    }

    public mutating func next(read: (UnsafeMutableRawBufferPointer) throws -> Int) throws -> Data? {
        guard !finished else { return nil }
        var line = Data()
        do {
            while true {
                if consumed == available {
                    consumed = 0
                    available = try buffer.withUnsafeMutableBytes(read)
                    guard available >= 0, available <= buffer.count else { throw Failure.invalidRead }
                    if available == 0 {
                        finished = true
                        return line.isEmpty ? nil : line
                    }
                }
                let delimiter = buffer[consumed..<available].firstIndex(of: 10)
                let end = delimiter ?? available
                guard end - consumed <= limit - line.count else { throw Failure.tooLarge }
                line.append(contentsOf: buffer[consumed..<end])
                consumed = end
                if delimiter != nil {
                    consumed += 1
                    return line
                }
            }
        } catch {
            finished = true
            throw error
        }
    }
}
