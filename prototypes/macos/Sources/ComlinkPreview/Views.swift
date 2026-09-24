import SwiftUI
import PrototypeCore

struct Surface<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        content.padding(22)
            .background(.regularMaterial)
            .background(Color(nsColor: .windowBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 18))
            .overlay(RoundedRectangle(cornerRadius: 18).stroke(.primary.opacity(0.12), lineWidth: 1))
    }
}

struct LocalFooter: View {
    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "waveform").accessibilityHidden(true)
            Text("Comlink")
            Spacer()
            Circle().fill(.green).frame(width: 5, height: 5).accessibilityHidden(true)
            Text("Synthetic preview · No audio captured")
        }.font(.system(size: 11)).foregroundStyle(.secondary)
    }
}

struct GuideView: View {
    @ObservedObject var model: PreviewModel
    var body: some View {
        Surface {
            VStack(alignment: .leading, spacing: 20) {
                HStack {
                    Image(systemName: "waveform").font(.title2)
                    Text("Comlink").font(.title2.weight(.semibold))
                    Spacer()
                    Text("DESIGN PREVIEW").font(.system(size: 10, weight: .semibold)).tracking(1.4).foregroundStyle(.secondary)
                }
                VStack(alignment: .leading, spacing: 7) {
                    Text("Present only when you speak.").font(.system(size: 23, weight: .medium))
                    Text("A quiet recording pill. A command palette when you need it.")
                        .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
                Divider()
                Picker("Appearance", selection: $model.appearance) {
                    ForEach(["System", "Light", "Dark"], id: \.self) { Text($0).tag($0) }
                }.pickerStyle(.segmented)
                    .onChange(of: model.appearance) { _, value in AppearanceAdapter().apply(value) }
                Picker("Preview scenario", selection: $model.scenario) {
                    ForEach(Scenario.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
                Picker("Work mode", selection: $model.mode) {
                    ForEach(WorkMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }.pickerStyle(.segmented)
                HStack {
                    Button("Start preview", action: model.start).buttonStyle(.borderedProminent)
                    Button("Open command palette", action: model.openPalette)
                    Spacer()
                }
                Text("Start from here or the menu-bar waveform. Stop with the square in the pill. The palette stays closed throughout dictation.")
                    .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                Divider()
                Text("Sample text only. Copy uses your clipboard. Insert demonstrates a recoverable failure; no text is sent to another app. Shortcuts apply only while this preview is focused.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                LocalFooter()
            }
        }.frame(width: 550)
    }
}

struct PillView: View {
    @ObservedObject var model: PreviewModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private let heights: [CGFloat] = [5, 10, 17, 9, 26, 35, 20, 12, 29, 41, 25, 15, 8, 19, 12, 6, 10]

    var body: some View {
        HStack(spacing: 17) {
            Image(systemName: "mic.fill").font(.system(size: 19))
                .frame(width: 38, height: 38).background(.white.opacity(0.08), in: Circle())
                .accessibilityHidden(true)
            if model.session.stage == .listening {
                TimelineView(.periodic(from: .now, by: reduceMotion ? 1 : 0.25)) { context in
                    HStack(spacing: 3) {
                        ForEach(heights.indices, id: \.self) { i in
                            Capsule().fill(.white.opacity(i > 12 ? 0.45 : 0.9))
                                .frame(width: 3, height: heights[i] * (reduceMotion ? 0.7 : (0.65 + 0.3 * sin(context.date.timeIntervalSince1970 * 4 + Double(i)))))
                        }
                    }.frame(width: 100, height: 42)
                }.accessibilityLabel("Listening, simulated waveform")
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    let elapsed = max(0, Int(context.date.timeIntervalSince(model.startedAt)))
                    Text(String(format: "%d:%02d", elapsed / 60, elapsed % 60))
                        .monospacedDigit().font(.system(size: 14)).frame(width: 44)
                        .accessibilityLabel("Elapsed time \(elapsed) seconds")
                }
                Button(action: model.stop) {
                    Image(systemName: "stop.fill").frame(width: 32, height: 32)
                }.buttonStyle(.plain).background(.white.opacity(0.12), in: Circle())
                    .accessibilityLabel("Stop dictation").help("Stop and process sample")
            } else {
                Image(systemName: "waveform").accessibilityHidden(true)
                Text("Processing locally…").font(.system(size: 13)).frame(width: 171, alignment: .leading)
            }
            Button("esc", action: model.cancel)
                .font(.system(size: 11)).buttonStyle(.plain).foregroundStyle(.white.opacity(0.7))
                .fixedSize().frame(width: 22)
                .accessibilityLabel("Cancel dictation").help("Cancel; Escape when focused")
        }
        .padding(.horizontal, 20).frame(width: 420, height: 72)
        .foregroundStyle(.white)
        .background(Color(white: 0.12), in: Capsule())
        .overlay(Capsule().stroke(.white.opacity(0.22), lineWidth: 1))
        .padding(4)
        .onExitCommand(perform: model.cancel)
    }
}

struct TranscriptView: View {
    @ObservedObject var model: PreviewModel
    var body: some View {
        VStack(alignment: .leading, spacing: 15) {
            Picker("Transcript version", selection: $model.showOriginal) {
                Text("Cleaned").tag(false)
                Text("Original").tag(true)
            }.pickerStyle(.segmented)
            Text(model.showOriginal ? model.session.original : model.session.cleaned)
                .font(.system(size: 18)).foregroundStyle(.primary).lineSpacing(5).textSelection(.enabled)
                .frame(maxWidth: .infinity, minHeight: 78, alignment: .topLeading)
                .padding(14).background(.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 8))
            HStack {
                Text("\(model.session.mode.rawValue) · Sample transcript").font(.caption).foregroundStyle(.secondary)
                Spacer()
                if model.copied { Label("Copied", systemImage: "checkmark").font(.caption).foregroundStyle(.secondary) }
            }
            if model.copyError { Text("Copy failed. Select the text above and copy manually.").font(.caption).foregroundStyle(.red) }
            HStack {
                Button("Copy", action: model.copy).keyboardShortcut("c", modifiers: .command)
                    .buttonStyle(.borderedProminent)
                Button("Try Insert…", action: model.insert).help("Simulates insertion failure; never sends text")
                Spacer()
            }
        }
    }
}

struct ResultView: View {
    @ObservedObject var model: PreviewModel
    var body: some View {
        Surface {
            VStack(alignment: .leading, spacing: 15) {
                HStack {
                    Image(systemName: model.session.hasTranscript ? "text.alignleft" : "info.circle")
                    Text(model.stageName).font(.headline)
                    Spacer()
                    Button(action: model.reset) { Image(systemName: "xmark") }
                        .buttonStyle(.plain).accessibilityLabel("Dismiss result")
                }
                if model.session.stage == .failed(.insertion) {
                    Text("The previous text field is unavailable. Your text is safe here. Copy it, then paste where you need it.")
                        .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
                if model.session.hasTranscript {
                    TranscriptView(model: model)
                } else {
                    Text(explanation).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    HStack {
                        Button(recovery, action: model.retry).buttonStyle(.borderedProminent)
                        Button("Preview controls", action: model.openGuide)
                    }
                }
                Divider()
                LocalFooter()
            }
        }.frame(width: 450)
        .onExitCommand(perform: model.reset)
    }

    private var explanation: String {
        switch model.session.stage {
        case .failed(.microphone): return "Enable Comlink in System Settings → Privacy & Security → Microphone, then try again. This preview does not request permission."
        case .failed(.model): return "Select an installed Whisper model with comlink models select, then retry. Audio stays on this Mac; there is no cloud fallback."
        case .failed(.silence): return "Try again closer to your microphone and check your input device. No transcript was saved."
        case .canceled: return "The sample was discarded. Nothing was copied or saved."
        default: return "Start a new sample to continue."
        }
    }
    private var recovery: String {
        switch model.session.stage {
        case .failed(.microphone): return "Simulate access granted"
        case .failed(.model): return "Simulate model selected"
        default: return "Try again"
        }
    }
}

struct PaletteView: View {
    @ObservedObject var model: PreviewModel
    @State private var query = ""
    @State private var selection = 0
    @State private var detail: PaletteAction?
    @FocusState private var searchFocused: Bool
    private var actions: [PaletteAction] { PaletteAction.matching(query) }

    var body: some View {
        Surface {
            VStack(alignment: .leading, spacing: 16) {
                if let detail {
                    HStack {
                        Button { self.detail = nil; searchFocused = true } label: { Image(systemName: "arrow.left") }
                            .buttonStyle(.plain).accessibilityLabel("Back to actions")
                        Text(detail.rawValue).font(.headline)
                        Spacer()
                        Button("Close", action: model.closePalette).buttonStyle(.plain)
                    }
                    if detail == .review && model.session.hasTranscript {
                        TranscriptView(model: model)
                    } else {
                        detailContent(detail)
                    }
                } else {
                    HStack(spacing: 12) {
                        Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                        TextField("What would you like to do?", text: $query)
                            .textFieldStyle(.plain).font(.system(size: 17)).focused($searchFocused)
                            .accessibilityLabel("Search commands")
                            .onSubmit { activateSelected() }
                        Button("esc", action: model.closePalette).buttonStyle(.plain).foregroundStyle(.secondary)
                            .accessibilityLabel("Close command palette")
                    }.padding(.bottom, 5)
                    Divider()
                    VStack(spacing: 4) {
                        ForEach(Array(actions.enumerated()), id: \.element) { index, action in
                            Button { activate(action) } label: {
                                HStack(spacing: 13) {
                                    Image(systemName: symbol(action)).frame(width: 20)
                                    Text(action.rawValue)
                                    Spacer()
                                    if selection == index { Text("↵").foregroundStyle(.secondary) }
                                }.padding(.horizontal, 12).frame(height: 42)
                                    .contentShape(Rectangle())
                            }.buttonStyle(.plain)
                                .background(selection == index ? Color.accentColor.opacity(0.16) : .clear, in: RoundedRectangle(cornerRadius: 8))
                                .accessibilityAddTraits(selection == index ? .isSelected : [])
                        }
                        if actions.isEmpty { Text("No matching commands").foregroundStyle(.secondary).padding(20) }
                    }
                    Text("WORK MODE").font(.system(size: 10, weight: .semibold)).tracking(1).foregroundStyle(.secondary)
                    Picker("Work mode", selection: $model.mode) {
                        ForEach(WorkMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                    }.pickerStyle(.segmented)
                    Text("↑ ↓ Navigate   ↵ Open   ⌘K Palette (in preview)").font(.caption).foregroundStyle(.secondary)
                }
                Divider()
                LocalFooter()
            }.frame(minHeight: 360, alignment: .topLeading)
        }.frame(width: 550)
        .onAppear { searchFocused = true }
        .onChange(of: query) { _, _ in selection = 0 }
        .onKeyPress(.downArrow) {
            guard detail == nil else { return .ignored }
            selection = PaletteAction.move(selection, by: 1, count: actions.count); return .handled
        }
        .onKeyPress(.upArrow) {
            guard detail == nil else { return .ignored }
            selection = PaletteAction.move(selection, by: -1, count: actions.count); return .handled
        }
        .onExitCommand(perform: goBack)
        .onChange(of: model.paletteEscape) { _, _ in goBack() }
    }

    private func goBack() {
        if detail != nil { detail = nil; searchFocused = true } else { model.closePalette() }
    }
    private func activateSelected() {
        guard actions.indices.contains(selection) else { return }
        activate(actions[selection])
    }
    private func activate(_ action: PaletteAction) {
        if action == .dictate { model.start() } else { detail = action }
    }
    private func symbol(_ action: PaletteAction) -> String {
        switch action {
        case .dictate: return "mic"
        case .file: return "doc.badge.plus"
        case .history: return "clock"
        case .vocabulary: return "book"
        case .review: return "text.alignleft"
        }
    }
    @ViewBuilder private func detailContent(_ action: PaletteAction) -> some View {
        VStack(alignment: .leading, spacing: 15) {
            switch action {
            case .file:
                Text("Bring your audio. Keep it local.").font(.title3)
                Text("The first app slice will hand a selected file to Comlink's existing transcribe pipeline. This preview uses a sample and does not read files.").foregroundStyle(.secondary)
                Button("Preview sample file") { model.start(); if model.session.stage == .listening { model.stop() } }
            case .history:
                Text("History follows your retention settings.").font(.title3)
                Text("No saved history is read or written by this preview.").foregroundStyle(.secondary)
                if model.session.hasTranscript { Button("Review current sample") { detail = .review } }
                else { Text("No sample yet. Start dictation to create one.").foregroundStyle(.secondary) }
            case .vocabulary:
                Text("Your words, preserved.").font(.title3)
                Text("Production uses the existing vocab and snippets commands. Example entries below are illustrative.").foregroundStyle(.secondary)
                LabeledContent("Vocabulary", value: "super base → Supabase")
                LabeledContent("Snippet", value: "my signature → Best, Luke")
            default:
                Text("No transcript yet.").font(.title3)
                Button("Start dictation", action: model.start)
            }
        }.fixedSize(horizontal: false, vertical: true).frame(minHeight: 180, alignment: .topLeading)
    }
}
