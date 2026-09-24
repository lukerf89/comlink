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

    func start() {
        copied = false
        copyError = false
        showOriginal = false
        startedAt = Date()
        session.start(scenario: scenario, mode: mode)
        closePalette()
        render()
    }

    func stop() {
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

    func cancel() { session.cancel(); render() }
    func reset() { session.reset(); render() }
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
