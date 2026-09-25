import Foundation
import PrototypeCore

func runHotkeyTests() {
    // Defaults and persisted round trip; malformed timing falls back predictably.
    var settings = HotkeySettings()
    expectEqual(settings.key, .function)
    expectEqual(settings.doublePressWindow, 0.35)
    expectTrue(settings.enabled)
    expectFalse(settings.acrossApps)
    settings.key = .rightOption
    expectEqual(try! JSONDecoder().decode(HotkeySettings.self, from: JSONEncoder().encode(settings)), settings)
    settings.doublePressWindow = -1
    expectEqual(settings.validated().doublePressWindow, 0.35)

    // Hold records on down, stops on release, with no extra delay after a long hold.
    var gesture = RecordingGesture()
    expectEqual(gesture.press(at: 1), [.start])
    expectEqual(gesture.press(at: 1.1), [])
    expectEqual(gesture.release(at: 3), [.stop])
    expectEqual(gesture.release(at: 3.1), [])
    expectEqual(gesture.phase, .idle)

    // Double Fn keeps a single recording alive through both releases; next down stops.
    gesture = RecordingGesture()
    expectEqual(gesture.press(at: 1), [.start])
    expectEqual(gesture.release(at: 1.05), [])
    expectEqual(gesture.phase, .awaitingSecondPress)
    expectEqual(gesture.expire(at: 1.1), [])
    expectEqual(gesture.press(at: 1.2), [])
    expectEqual(gesture.phase, .locked)
    expectEqual(gesture.release(at: 1.25), [])
    expectEqual(gesture.expire(at: 2), [])
    expectEqual(gesture.phase, .locked)
    expectEqual(gesture.press(at: 4), [.stop])
    expectEqual(gesture.release(at: 4.1), [])
    expectEqual(gesture.phase, .idle)

    // A short single tap waits only until the original down + double window.
    gesture = RecordingGesture()
    _ = gesture.press(at: 0)
    _ = gesture.release(at: 0.1)
    expectEqual(gesture.deadline, 0.35)
    expectEqual(gesture.expire(at: 0.34), [])
    expectEqual(gesture.expire(at: 0.35), [.stop])
    expectEqual(gesture.expire(at: 1), [])

    // Late presses cannot merge sessions or lock a completed first tap.
    gesture = RecordingGesture()
    _ = gesture.press(at: 0)
    _ = gesture.release(at: 0.1)
    expectEqual(gesture.press(at: 0.36), [.stop])
    expectEqual(gesture.phase, .idle)

    // Cancellation/settings changes invalidate pending release and timeout work.
    for lock in [false, true] {
        gesture = RecordingGesture()
        _ = gesture.press(at: 0)
        _ = gesture.release(at: 0.1)
        if lock { _ = gesture.press(at: 0.2) }
        gesture.reset()
        expectEqual(gesture.release(at: 0.3), [])
        expectEqual(gesture.expire(at: 1), [])
        expectEqual(gesture.press(at: 2), [.start])
    }

    // Window preference and inclusive boundary behavior.
    gesture = RecordingGesture(window: 0.5)
    _ = gesture.press(at: 0)
    _ = gesture.release(at: 0.1)
    _ = gesture.press(at: 0.5)
    expectEqual(gesture.phase, .locked)
    expectEqual(RecordingKey.function.keyCode, 63)
    expectEqual(RecordingKey.rightOption.keyCode, 61)
    // Real UserDefaults persistence in a disposable suite, including corrupt data.
    let suiteName = "dev.comlink.preview-tests.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suiteName)!
    defer { defaults.removePersistentDomain(forName: suiteName) }
    let preferences = HotkeyPreferenceAdapter(defaults: defaults)
    expectEqual(preferences.load(), HotkeySettings())
    var saved = HotkeySettings()
    saved.key = .rightOption
    saved.doublePressWindow = 0.5
    saved.enabled = false
    preferences.save(saved)
    expectEqual(HotkeyPreferenceAdapter(defaults: defaults).load(), saved)
    defaults.set(Data("broken".utf8), forKey: "recording-hotkey-v1")
    expectEqual(preferences.load(), HotkeySettings())
    print("PASS: 8 hotkey test groups (settings, hold, lock, single tap, late press, reset, timing, persistence)")
}
