import Foundation

public struct WindowPointerAnchor: Equatable, Sendable {
    public let pid: Int32
    public let windowID: UInt32
    public let bounds: CGRect
    public let point: CGPoint

    public init?(pid: Int32, windowID: UInt32, bounds: CGRect, point: CGPoint) {
        guard pid > 0, windowID > 0,
              [bounds.minX, bounds.minY, bounds.width, bounds.height, point.x, point.y].allSatisfy(\.isFinite),
              bounds.width > 0, bounds.height > 0, bounds.contains(point) else { return nil }
        self.pid = pid
        self.windowID = windowID
        self.bounds = bounds
        self.point = point
    }

    public struct Window: Equatable, Sendable {
        public let pid: Int32
        public let id: UInt32
        public let bounds: CGRect
        public let alpha: Double

        public init(pid: Int32, id: UInt32, bounds: CGRect, alpha: Double) {
            self.pid = pid
            self.id = id
            self.bounds = bounds
            self.alpha = alpha
        }
    }

    public func visible(in orderedWindows: [Window], ignoringWindowID: UInt32?) -> Bool {
        for window in orderedWindows {
            if window.id == ignoringWindowID { continue }
            if window.id == windowID {
                return window.pid == pid && window.bounds == bounds && window.alpha > 0
            }
            if window.alpha > 0 && window.bounds.contains(point) {
                return false
            }
        }
        return false
    }

    public func panelFrame(mainDisplayHeight: Double, size: CGSize) -> CGRect? {
        guard mainDisplayHeight.isFinite, size.width.isFinite, size.height.isFinite,
              mainDisplayHeight > 0, size.width > 0, size.height > 0 else { return nil }
        return CGRect(x: point.x - 1, y: mainDisplayHeight - point.y - size.height + 1,
                      width: size.width, height: size.height)
    }
}
