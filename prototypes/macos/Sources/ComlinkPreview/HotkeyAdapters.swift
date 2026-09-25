import AppKit
import ApplicationServices
import PrototypeCore

/// Passive observation only: events are never swallowed or synthesized.
/// No event characters are read, stored, or logged.
@MainActor
final class HotkeyInputAdapter {
    private var localMonitor: Any?
    private var globalMonitor: Any?
    private var permissionTimer: Timer?
    private var settings = HotkeySettings()
    private var onChange: (Bool, TimeInterval) -> Void = { _, _ in }
    private var onInterrupt: () -> Void = {}
    private var onAccessLost: () -> Void = {}
    private(set) var observesAcrossApps = false

    func configure(_ settings: HotkeySettings,
                   onChange: @escaping (Bool, TimeInterval) -> Void,
                   onInterrupt: @escaping () -> Void,
                   onAccessLost: @escaping () -> Void) -> String {
        stop()
        self.settings = settings
        self.onChange = onChange
        self.onInterrupt = onInterrupt
        self.onAccessLost = onAccessLost
        guard settings.enabled else { return "Recording hotkey is off" }
        localMonitor = NSEvent.addLocalMonitorForEvents(matching: [.flagsChanged, .keyDown]) { [weak self] event in
            self?.receive(event)
            return event
        }
        if settings.acrossApps && AXIsProcessTrusted() {
            globalMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.flagsChanged, .keyDown]) { [weak self] event in
                self?.receive(event)
            }
            observesAcrossApps = globalMonitor != nil
            if observesAcrossApps {
                permissionTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
                    Task { @MainActor in _ = self?.checkAccess() }
                }
            }
        }
        if localMonitor == nil { return "Hotkey unavailable. Use the menu-bar controls." }
        if observesAcrossApps { return "Listening in this preview and other apps" }
        return settings.acrossApps
            ? "Accessibility access needed · Works in this preview only"
            : "Works while this preview is focused"
    }

    @discardableResult private func checkAccess() -> Bool {
        guard observesAcrossApps && !AXIsProcessTrusted() else { return false }
        // A revoked global monitor may stop receiving events entirely. Poll while
        // enabled so a locked sample cannot remain running solely for that reason.
        if let globalMonitor { NSEvent.removeMonitor(globalMonitor) }
        globalMonitor = nil
        permissionTimer?.invalidate()
        permissionTimer = nil
        observesAcrossApps = false
        onAccessLost()
        return true
    }

    private func receive(_ event: NSEvent) {
        if checkAccess() { return }
        // Fn combined with another key must not leave a hold-to-talk session running.
        // Look at event type only; never read the typed characters.
        if event.type == .keyDown {
            onInterrupt()
            return
        }
        guard event.keyCode == settings.key.keyCode else {
            if !event.modifierFlags.intersection([.command, .control, .shift, .option]).isEmpty {
                onInterrupt()
            }
            return
        }
        let down: Bool
        if settings.key == .function {
            down = event.modifierFlags.contains(.function)
        } else {
            // NX_DEVICERALTKEYMASK: avoid confusing left Option with right Option.
            down = event.modifierFlags.rawValue & 0x40 != 0
        }
        let forbidden: NSEvent.ModifierFlags = settings.key == .function
            ? [.command, .control, .shift, .option] : [.command, .control, .shift, .function]
        if down && !event.modifierFlags.intersection(forbidden).isEmpty {
            onInterrupt()
            return
        }
        onChange(down, event.timestamp)
    }

    func stop() {
        permissionTimer?.invalidate()
        permissionTimer = nil
        if let localMonitor { NSEvent.removeMonitor(localMonitor) }
        if let globalMonitor { NSEvent.removeMonitor(globalMonitor) }
        localMonitor = nil
        globalMonitor = nil
        observesAcrossApps = false
    }

    func openAccessibilitySettings() {
        // Opening the pane does not grant access or prompt on application launch.
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }
}
