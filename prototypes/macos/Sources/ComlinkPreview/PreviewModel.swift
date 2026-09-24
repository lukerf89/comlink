import AppKit
import SwiftUI
import PrototypeCore

/// The only data-delivery adapter in the preview. Called exclusively by Copy.
struct ClipboardAdapter {
    func copy(_ text: String) -> Bool {
        NSPasteboard.general.clearContents()
        return NSPasteboard.general.setString(text, forType: .string)
    }
}

@MainActor
struct AppearanceAdapter {
    func apply(_ appearance: String) {
        NSApp.appearance = appearance == "System" ? nil : NSAppearance(named: appearance == "Light" ? .aqua : .darkAqua)
    }
}

@MainActor
final class PreviewModel: ObservableObject {
    @Published var session = Session()
    @Published var hotkeys = HotkeyPreferenceAdapter().load()
    @Published var gesture = RecordingGesture()
    @Published var hotkeyStatus = "Works while this preview is focused"
    var updateHotkeyInput: () -> Void = {}
    var openHotkeySettings: () -> Void = {}
    var openAccessibility: () -> Void = {}
    private var releaseTask: Task<Void, Never>?
    private var demoTask: Task<Void, Never>?
    var isHotkeyLocked: Bool { gesture.phase == .locked }

    func saveHotkeys() {
        cancelHotkeyForSettingsOrSleep()
        hotkeys = hotkeys.validated()
        HotkeyPreferenceAdapter().save(hotkeys)
        gesture = RecordingGesture(window: hotkeys.doublePressWindow)
        updateHotkeyInput()
    }

    func clearGesture() {
        releaseTask?.cancel()
        demoTask?.cancel()
        gesture.reset()
    }

    func interruptHotkey() {
        // Once locked, ordinary typing is allowed; explicit stop/cancel ends it.
        guard gesture.phase == .held || gesture.phase == .awaitingSecondPress else { return }
        cancel()
    }

    func cancelHotkeyForSettingsOrSleep() {
        if gesture.phase != .idle { cancel() }
        clearGesture()
    }

    func hotkeyChanged(down: Bool, at now: TimeInterval) {
        guard hotkeys.enabled, session.stage != .processing else { return }
        // A hotkey press also finishes a recording started from the menu or palette.
        if down && session.stage == .listening && gesture.phase == .idle {
            stop()
            return
        }
        let effects = down ? gesture.press(at: now) : gesture.release(at: now)
        applyHotkeyEffects(effects)
        releaseTask?.cancel()
        if let deadline = gesture.deadline {
            let token = session.token
            releaseTask = Task { [weak self] in
                try? await Task.sleep(for: .seconds(max(0, deadline - ProcessInfo.processInfo.systemUptime)))
                guard !Task.isCancelled, let self, self.session.token == token else { return }
                self.applyHotkeyEffects(self.gesture.expire(at: ProcessInfo.processInfo.systemUptime))
            }
        }
    }

    private func applyHotkeyEffects(_ effects: [HotkeyEffect]) {
        for effect in effects {
            switch effect {
            case .start:
                start(preservingGesture: true)
                if session.stage != .listening { clearGesture() }
            case .stop: stop()
            }
        }
    }

    func previewHold() {
        cancelHotkeyForSettingsOrSleep()
        let now = ProcessInfo.processInfo.systemUptime
        hotkeyChanged(down: true, at: now)
        demoTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled, let self else { return }
            self.hotkeyChanged(down: false, at: ProcessInfo.processInfo.systemUptime)
        }
    }

    func previewDoublePress() {
        cancelHotkeyForSettingsOrSleep()
        let now = ProcessInfo.processInfo.systemUptime
        hotkeyChanged(down: true, at: now)
        hotkeyChanged(down: false, at: now + 0.01)
        hotkeyChanged(down: true, at: now + 0.02)
        hotkeyChanged(down: false, at: now + 0.03)
    }
    @Published var appearance = "System"
    @Published var paletteEscape = 0
    @Published var scenario: Scenario = .success
    @Published var mode: WorkMode = .clean
    @Published var copied = false
    @Published var copyError = false
    @Published var showOriginal = false
    @Published var startedAt = Date()
    var render: () -> Void = {}
    var openPalette: () -> Void = {}
    var closePalette: () -> Void = {}
    var openGuide: () -> Void = {}

    func start() { start(preservingGesture: false) }

    private func start(preservingGesture: Bool) {
        if !preservingGesture { clearGesture() }
        copied = false
        copyError = false
        showOriginal = false
        startedAt = Date()
        session.start(scenario: scenario, mode: mode)
        closePalette()
        render()
    }

    func stop() {
        guard session.stage == .listening else { return }
        clearGesture()
        session.stop()
        let token = session.token
        render()
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(1.6))
            guard let self else { return }
            self.session.finish(token: token)
            self.render()
        }
    }

    func cancel() { clearGesture(); session.cancel(); render() }
    func reset() { clearGesture(); session.reset(); render() }
    func insert() { session.simulateInsert(); render() }
    func retry() { scenario = .success; start() }
    func copy() {
        guard session.hasTranscript else { return }
        copied = ClipboardAdapter().copy(showOriginal ? session.original : session.cleaned)
        copyError = !copied
    }

    var stageName: String {
        switch session.stage {
        case .idle: return "Ready when you are"
        case .listening: return "Listening"
        case .processing: return "Processing on this Mac"
        case .completed: return "Ready to copy"
        case .canceled: return "Dictation canceled"
        case .failed(.microphone): return "Allow microphone access"
        case .failed(.model): return "Choose a local model"
        case .failed(.silence): return "No speech detected"
        case .failed(.insertion): return "Couldn't insert text"
        case .failed(.success): return "Try again"
        }
    }
}
