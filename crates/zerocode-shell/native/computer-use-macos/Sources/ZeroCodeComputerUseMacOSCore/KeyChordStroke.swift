import Foundation

/// One key chord as the hand posts it — the one sequence the helper's key verbs
/// (`key`, `holdKey`) and a reflex key leaf all post (t-10384): each modifier
/// down with the flags held so far, the key down under all of them, then the
/// key up and each modifier up, newest first, with the flags still held after
/// it. The helper's own key table (`KeyMap`) names the codes and the flags.
public struct KeyChordStroke: Equatable, Sendable {
    public struct Modifier: Equatable, Sendable {
        public let code: UInt16
        public let flag: UInt64

        public init(code: UInt16, flag: UInt64) {
            self.code = code
            self.flag = flag
        }
    }

    public let key: UInt16
    public let modifiers: [Modifier]

    public init(key: UInt16, modifiers: [Modifier]) {
        self.key = key
        self.modifiers = modifiers
    }

    /// Every flag the chord holds while its key is down.
    public var flags: UInt64 { modifiers.reduce(UInt64(0)) { $0 | $1.flag } }

    /// The presses, in order: each modifier, then the key.
    public func downs(route: HandRoute, source: HandSource = .none) -> [HandEvent] {
        var held: UInt64 = 0
        var events: [HandEvent] = []
        for modifier in modifiers {
            held |= modifier.flag
            events.append(HandEvent(.key(code: modifier.code, down: true, modifier: modifier.flag), flags: held, route: route, source: source))
        }
        events.append(HandEvent(.key(code: key, down: true, modifier: 0), flags: held, route: route, source: source))
        return events
    }

    /// The releases, in order: the key, then each modifier, newest first.
    public func ups(route: HandRoute, source: HandSource = .none) -> [HandEvent] {
        var held = flags
        var events = [HandEvent(.key(code: key, down: false, modifier: 0), flags: held, route: route, source: source)]
        for modifier in modifiers.reversed() {
            held &= ~modifier.flag
            events.append(HandEvent(.key(code: modifier.code, down: false, modifier: 0), flags: held, route: route, source: source))
        }
        return events
    }
}
