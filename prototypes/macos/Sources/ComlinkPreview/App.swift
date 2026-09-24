import AppKit
import SwiftUI
import PrototypeCore

final class FloatingPanel: NSPanel {
    var onEscape: () -> Void = {}
    override func cancelOperation(_ sender: Any?) { onEscape() }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { onEscape() } else { super.keyDown(with: event) }
    }
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = PreviewModel()
    var status: NSStatusItem!
    var guide: NSPanel!
    var pill: NSPanel!
    var result: NSPanel!
    var palette: NSPanel!
    var hotkeySettings: NSPanel!
    let hotkeyInput = HotkeyInputAdapter()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        let menu = NSMenu()
        menu.addItem(item("Start / stop dictation preview", #selector(toggle)))
        menu.addItem(item("Stop dictation", #selector(stop)))
        menu.addItem(item("Cancel dictation", #selector(cancel)))
        menu.addItem(.separator())
        menu.addItem(item("Command palette…", #selector(showPalette), key: "k"))
        menu.addItem(item("Hotkey settings…", #selector(showHotkeys), key: ","))
        menu.addItem(item("Preview controls…", #selector(showGuide)))
        menu.addItem(.separator())
        menu.addItem(item("Quit Comlink Preview", #selector(quit), key: "q"))
        status = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        status.button?.image = NSImage(systemSymbolName: "waveform", accessibilityDescription: "Comlink preview")
        status.button?.toolTip = "Comlink — design preview"
        status.menu = menu
        // A main menu makes the same shortcuts available while preview windows are focused.
        let mainMenu = NSMenu()
        let appItem = NSMenuItem()
        appItem.submenu = menu.copy() as? NSMenu
        mainMenu.addItem(appItem)
        let editItem = NSMenuItem(title: "Edit", action: nil, keyEquivalent: "")
        let edit = NSMenu(title: "Edit")
        for (title, action, key) in [("Copy", "copy:", "c"), ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a")] {
            edit.addItem(NSMenuItem(title: title, action: Selector(action), keyEquivalent: key))
        }
        editItem.submenu = edit
        mainMenu.addItem(editItem)
        NSApp.mainMenu = mainMenu

        guide = panel(GuideView(model: model), title: "Comlink Preview Controls")
        pill = panel(PillView(model: model), title: "Comlink Recording Pill")
        result = panel(ResultView(model: model), title: "Comlink Result")
        palette = panel(PaletteView(model: model), title: "Comlink Command Palette")
        hotkeySettings = panel(HotkeySettingsView(model: model), title: "Comlink Hotkey Settings")
        (hotkeySettings as? FloatingPanel)?.onEscape = { [weak self] in self?.hotkeySettings.orderOut(nil) }
        (guide as? FloatingPanel)?.onEscape = { [weak self] in self?.guide.orderOut(nil) }
        (pill as? FloatingPanel)?.onEscape = { [weak self] in self?.model.cancel() }
        (result as? FloatingPanel)?.onEscape = { [weak self] in self?.model.reset() }
        (palette as? FloatingPanel)?.onEscape = { [weak self] in self?.model.paletteEscape += 1 }
        model.render = { [weak self] in self?.render() }
        model.openPalette = { [weak self] in self?.showPalette() }
        model.closePalette = { [weak self] in self?.palette.orderOut(nil) }
        model.openGuide = { [weak self] in self?.showGuide() }
        model.openHotkeySettings = { [weak self] in self?.showHotkeys() }
        model.updateHotkeyInput = { [weak self] in self?.configureHotkeys() }
        model.openAccessibility = { [weak self] in self?.hotkeyInput.openAccessibilitySettings() }
        model.gesture = RecordingGesture(window: model.hotkeys.doublePressWindow)
        NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(willSleep), name: NSWorkspace.willSleepNotification, object: nil)
        configureHotkeys()
        showGuide()
    }

    private func configureHotkeys() {
        model.cancelHotkeyForSettingsOrSleep()
        model.hotkeyStatus = hotkeyInput.configure(model.hotkeys, onChange: { [weak self] down, time in
            self?.model.hotkeyChanged(down: down, at: time)
        }, onInterrupt: { [weak self] in self?.model.interruptHotkey() }, onAccessLost: { [weak self] in
            self?.model.cancelHotkeyForSettingsOrSleep()
            self?.model.hotkeyStatus = "Accessibility access lost · Recheck access in Hotkey settings"
        })
    }

    func applicationDidResignActive(_ notification: Notification) {
        if !hotkeyInput.observesAcrossApps { model.cancelHotkeyForSettingsOrSleep() }
    }
    func applicationWillTerminate(_ notification: Notification) {
        hotkeyInput.stop()
        model.cancelHotkeyForSettingsOrSleep()
        NSWorkspace.shared.notificationCenter.removeObserver(self)
    }
    @objc private func willSleep() { model.cancelHotkeyForSettingsOrSleep() }
    @objc private func showHotkeys() {
        position(hotkeySettings)
        NSApp.activate(ignoringOtherApps: true)
        hotkeySettings.makeKeyAndOrderFront(nil)
    }

    private func item(_ title: String, _ action: Selector, key: String = "", modifiers: NSEvent.ModifierFlags = .command) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.target = self
        item.keyEquivalentModifierMask = modifiers
        return item
    }

    private func panel<V: View>(_ view: V, title: String) -> NSPanel {
        let panel = FloatingPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.title = title
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.contentView = NSHostingView(rootView: view)
        panel.setContentSize(panel.contentView!.fittingSize)
        panel.isMovableByWindowBackground = true
        return panel
    }

    private func position(_ panel: NSPanel, bottom: Bool = false) {
        panel.setContentSize(panel.contentView!.fittingSize)
        let screen = (NSScreen.main ?? NSScreen.screens[0]).visibleFrame
        panel.setFrameOrigin(NSPoint(x: screen.midX - panel.frame.width / 2,
                                     y: bottom ? screen.minY + 40 : screen.midY - panel.frame.height / 2))
    }

    private func render() {
        pill.orderOut(nil)
        result.orderOut(nil)
        if model.session.stage != .idle { guide.orderOut(nil); hotkeySettings.orderOut(nil) }
        switch model.session.stage {
        case .idle: break
        case .listening, .processing:
            position(pill, bottom: true)
            // Does not activate Comlink or take keyboard focus from the current app.
            pill.orderFrontRegardless()
        default:
            result.contentView = NSHostingView(rootView: ResultView(model: model))
            position(result, bottom: true)
            result.orderFrontRegardless()
        }
    }

    @objc func toggle() {
        if model.session.stage == .listening { model.stop() }
        else if model.session.stage != .processing { model.start() }
    }
    @objc func stop() { if model.session.stage == .listening { model.stop() } }
    @objc func cancel() { model.cancel() }
    @objc func showPalette() {
        palette.contentView = NSHostingView(rootView: PaletteView(model: model))
        position(palette)
        NSApp.activate(ignoringOtherApps: true)
        palette.makeKeyAndOrderFront(nil)
    }
    @objc func showGuide() {
        position(guide)
        NSApp.activate(ignoringOtherApps: true)
        guide.makeKeyAndOrderFront(nil)
    }
    @objc func quit() { NSApp.terminate(nil) }
}

@main
struct PreviewApp {
    @MainActor static func main() {
        if CommandLine.arguments.contains("--check-startup") {
            let model = PreviewModel()
            precondition(RecordingKey.allCases.contains(model.hotkeys.key))
            print("{\"startup\":\"ok\"}")
            return
        }
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        withExtendedLifetime(delegate) { app.run() }
    }
}
