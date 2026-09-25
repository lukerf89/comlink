import Foundation

/// Only preview preferences, never CLI configuration or transcript data.
public struct HotkeyPreferenceAdapter {
    private let defaults: UserDefaults
    public init(defaults: UserDefaults = .standard) { self.defaults = defaults }
    private let key = "recording-hotkey-v1"
    public func load() -> HotkeySettings {
        guard let data = defaults.data(forKey: key),
              let settings = try? JSONDecoder().decode(HotkeySettings.self, from: data) else {
            return HotkeySettings()
        }
        return settings.validated()
    }
    public func save(_ settings: HotkeySettings) {
        if let data = try? JSONEncoder().encode(settings.validated()) {
            defaults.set(data, forKey: key)
        }
    }
}
