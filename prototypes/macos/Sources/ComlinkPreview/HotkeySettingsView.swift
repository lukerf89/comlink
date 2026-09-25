import SwiftUI
import PrototypeCore

struct HotkeySettingsView: View {
    @ObservedObject var model: PreviewModel
    var body: some View {
        Surface {
            VStack(alignment: .leading, spacing: 18) {
                HStack {
                    Image(systemName: "keyboard").font(.title2)
                    Text("Hotkeys").font(.title2.weight(.semibold))
                    Spacer()
                    Text("COMLINK PREVIEW").font(.system(size: 10, weight: .semibold))
                        .tracking(1).foregroundStyle(.secondary)
                }
                Toggle("Recording hotkey", isOn: $model.hotkeys.enabled)
                Picker("Recording key", selection: $model.hotkeys.key) {
                    ForEach(RecordingKey.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }.disabled(!model.hotkeys.enabled)
                VStack(alignment: .leading, spacing: 10) {
                    instruction("Hold \(model.hotkeys.key.label)", "Record while held. Release to finish.", "mic")
                    instruction("Double-press \(model.hotkeys.key.label)", "Lock recording on. Release your hands.", "lock")
                    instruction("Press \(model.hotkeys.key.label) again", "Finish the locked recording.", "stop")
                }.padding(14).background(.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 10))
                Picker("Double-press speed", selection: $model.hotkeys.doublePressWindow) {
                    Text("Fast · 250 ms").tag(0.25)
                    Text("Normal · 350 ms").tag(0.35)
                    Text("Relaxed · 500 ms").tag(0.5)
                }.disabled(!model.hotkeys.enabled)
                VStack(alignment: .leading, spacing: 8) {
                    Toggle("Use while other apps are focused", isOn: $model.hotkeys.acrossApps)
                        .disabled(!model.hotkeys.enabled)
                    Text(model.hotkeyStatus).font(.caption).foregroundStyle(.secondary)
                    if model.hotkeys.acrossApps && model.hotkeys.enabled {
                        HStack {
                            Button("Open Accessibility settings", action: model.openAccessibility)
                            Button("Recheck access", action: model.updateHotkeyInput)
                        }
                    }
                }
                Divider()
                Text("Using Fn / Globe? In macOS Keyboard settings, choose “Do Nothing” for the Globe key and remove any double-Fn Dictation shortcut. Otherwise macOS may also trigger its own action.")
                    .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button("Preview hold", action: model.previewHold)
                    Button("Preview double-press", action: model.previewDoublePress)
                    if model.isHotkeyLocked {
                        Button("Preview next press") {
                            model.hotkeyChanged(down: true, at: ProcessInfo.processInfo.systemUptime)
                        }
                    }
                }.disabled(!model.hotkeys.enabled || model.session.stage == .processing)
                Text("These buttons demonstrate the gestures with sample text. Settings save automatically. Changing a setting cancels an active hotkey recording.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                LocalFooter()
            }
        }.frame(width: 570)
        .onChange(of: model.hotkeys) { _, _ in model.saveHotkeys() }
    }

    private func instruction(_ title: String, _ detail: String, _ symbol: String) -> some View {
        HStack(spacing: 12) {
            Image(systemName: symbol).frame(width: 18).foregroundStyle(.secondary).accessibilityHidden(true)
            Text(title).font(.system(size: 12, weight: .medium)).frame(width: 145, alignment: .leading)
            Text(detail).font(.system(size: 12)).foregroundStyle(.secondary)
        }
    }
}
