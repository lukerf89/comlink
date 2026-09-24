import Foundation

public enum RecordingKey: String, CaseIterable, Codable {
    case function = "Fn / Globe"
    case rightOption = "Right Option"
    public var label: String { self == .function ? "Fn" : "⌥" }
    public var keyCode: UInt16 { self == .function ? 63 : 61 }
}

public struct HotkeySettings: Codable, Equatable {
    public var enabled = true
    public var key: RecordingKey = .function
    public var doublePressWindow = 0.35
    public var acrossApps = false
    public init() {}

    public func validated() -> Self {
        var value = self
        if ![0.25, 0.35, 0.5].contains(value.doublePressWindow) {
            value.doublePressWindow = 0.35
        }
        return value
    }
}

public enum HotkeyEffect: Equatable { case start, stop }

/// Monotonic timestamps; no OS input, timers, audio or persistence.
public struct RecordingGesture {
    public enum Phase: Equatable { case idle, held, awaitingSecondPress, locked }
    public private(set) var phase: Phase = .idle
    public private(set) var isDown = false
    public private(set) var deadline: TimeInterval?
    private var firstDown: TimeInterval = 0
    public let window: TimeInterval

    public init(window: TimeInterval = 0.35) { self.window = window }

    public mutating func press(at now: TimeInterval) -> [HotkeyEffect] {
        guard !isDown else { return [] }
        isDown = true
        switch phase {
        case .idle:
            firstDown = now
            phase = .held
            return [.start]
        case .awaitingSecondPress:
            if let deadline, now <= deadline {
                phase = .locked
                self.deadline = nil
                return []
            }
            // A late second press stops the old sample; processing is never restarted.
            phase = .idle
            deadline = nil
            return [.stop]
        case .locked:
            phase = .idle
            return [.stop]
        case .held: return []
        }
    }

    public mutating func release(at now: TimeInterval) -> [HotkeyEffect] {
        guard isDown else { return [] }
        isDown = false
        guard phase == .held else { return [] }
        let end = firstDown + window
        if now >= end {
            phase = .idle
            return [.stop]
        }
        // Keep one sample alive during the double-press interval. Never transcribe
        // a first tap before a second tap has a chance to lock the same recording.
        phase = .awaitingSecondPress
        deadline = end
        return []
    }

    public mutating func expire(at now: TimeInterval) -> [HotkeyEffect] {
        guard phase == .awaitingSecondPress, let deadline, now >= deadline else { return [] }
        self.deadline = nil
        phase = .idle
        return [.stop]
    }

    public mutating func reset() {
        phase = .idle
        isDown = false
        deadline = nil
    }
}
