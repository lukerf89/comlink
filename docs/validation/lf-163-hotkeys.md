# LF-163 follow-up: Fn hotkey settings

2026-09-24. Luke approved the visual preview and requested Fn as the recording default, with double-Fn locking until another Fn press. Single-Fn is implemented as hold-to-talk. This extends PR #16; audio capture and transcription remain synthetic.

## Changes

- `RecordingHotkey.swift`: persisted setting schema, Fn/Right Option key definitions, and pure hold/double-press/locked/stop state machine with a monotonic deadline.
- `HotkeyPreferenceAdapter.swift`: standard macOS app preferences, with validated defaults and corrupt-data fallback; separate from CLI configuration.
- `HotkeyAdapters.swift`: passive local event observation; optional cross-app observation only after Accessibility is already granted. No typed characters are inspected, retained or logged. Modifier chords interrupt a hold, but ordinary typing does not end a locked session.
- `HotkeySettingsView.swift`, app/model/views: settings via gear/menu/palette/`⌘,`; enable toggle, key selector, timing selector, scope/status and manual access recheck, conflict guidance, gesture previews, lock icon and stop instruction.
- Tests and E2E: eight new hotkey check groups and a real app-executable startup smoke check that requires valid JSON and empty stderr. Existing six Swift groups and the Rust CLI remain intact.

## Observed validation

All commands exited 0:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all` — 98 passed (89 unit, 2 audio, 7 meeting lifecycle).
- `cargo run -- doctor` — local dependencies healthy; microphone permission remains a manual check.
- `scripts/e2e/lf-163-macos-prototype.sh` — 14 Swift check groups, app build, plist, executable, startup JSON/stderr, and isolated CLI config check passed.
- `scripts/dev/preview-macos.sh` — native app launched successfully after fixing the preferences initialization issue below.

The eight new automated groups cover defaults/encoding, hold/release and repeated events, double-press lock plus third-press stop, short single-tap expiry, late second press, cancellation/reset against stale events, configurable timing/boundaries, and real persistence/corrupt-data fallback in a disposable UserDefaults suite.

Native UI checks performed:

1. Opened settings with the gear and `⌘,`. Fn / Globe, enabled, 350 ms, local-only were the defaults; instructions and scope status were readable.
2. Preview double-press showed a lock icon and **Locked · Fn to stop**. The recording survived both simulated releases and remained locked while reopening settings.
3. Preview next press transitioned that same sample to processing, then completed.
4. Changed recording key to Right Option, quit/relaunched, and confirmed Right Option persisted. Restored Fn afterward.
5. Preview hold started listening, released through the same input model, and completed successfully.

Screenshots: `artifacts/lf-163/hotkey-settings.png` and `artifacts/lf-163/recording-locked.png` (synthetic content only).

## Regression found during launch

The initial preferences adapter attempted `UserDefaults(suiteName: <own bundle ID>)!`. macOS rejected the suite and the executable crashed at startup. Fixed by using `.standard`, plus dependency injection for tests. Added persistence/corruption regression coverage and `--check-startup` to the actual bundled executable. E2E now launches that path and fails on nonzero exit, unexpected stderr or invalid startup JSON. The corrected app was relaunched and preferences survived a real restart.

A later startup check exited 137/SIGKILL after the build script overwrote the signed executable in place while the earlier build was running. The packaging script now copies to a fresh temporary file and atomically renames it over the executable, avoiding stale Mach-O signature state, and ad-hoc signs the complete local app bundle. The signature check also caught the prior binary-only signature missing a bundle resource seal. E2E verifies the complete app signature and re-runs the actual startup check; the corrected suite passed with the prior app still running.

Quit the old preview before relaunching an updated build: macOS `open` can reuse the previous process even after replacing the executable. Initial UI-tool timeouts were investigated rather than reported as a successful launch.

## Boundaries and remaining manual checks

The UI automation tool rejects modifier-only key presses (`keyPressIncludedNoNonModifierKeys`), so the native physical Fn/Right Option delivery path is **not verified by automation**. Gesture buttons exercise the same controller but are not evidence of physical keyboard or cross-app event delivery. Accessibility access was not granted or changed by the agent, and the global monitor remains opt-in. Physical Fn, permission denial/grant/revocation, sleep, modifier chords and other-app focus behavior require the checklist below. The local app is ad-hoc signed for development; a stable Developer ID signed application bundle is still needed for a distributable release and durable permission identity.

While cross-app observation is enabled, a one-second permission check detects revocation even if the global monitor stops receiving events; this path still needs the manual grant/revocation test.

Passive monitoring does not suppress macOS or third-party shortcuts. Fn/Globe may trigger Emoji, input switching, or Apple's double-Fn Dictation as well; users must resolve those conflicts in Keyboard settings. External keyboards may handle Fn internally and never send a macOS modifier event. The preview's settings explain this limitation; no system settings are modified automatically.

The startup smoke test reads the app's preview preferences but does not write them. The settings UI persists only key, enabled state, timing and scope. No real transcript/audio history is retained; raw/cleaned sample text remains in memory. This follow-up does not advance a production phase or implement insertion.

## Manual gate

1. Run `scripts/dev/preview-macos.sh`, then open **Hotkey settings…** (`⌘,`). Confirm **Fn / Globe**, **Normal · 350 ms**, and enabled. Check macOS Keyboard settings for conflicting Fn/Globe and Dictation actions before testing real keys.
2. Hold physical Fn for a few seconds: expect a listening pill immediately. Release: expect processing and a result. Tap Fn once briefly: expect finish after at most the selected double-press interval, not an indefinitely running sample.
3. Press/release Fn twice within 350 ms: expect one continuous sample and **Locked · Fn to stop**. Wait several seconds without touching Fn; it must stay recording. Press Fn again: expect processing exactly once. The release after that must not start a new sample.
4. Repeat at 250 and 500 ms, including a second press just outside the interval. Repeat rapid taps during processing: no new sample may start until processing completes. Stop/Cancel must remain usable.
5. Test Fn+another key while holding: the sample must cancel. Test ordinary typing while locked: the sample stays locked. Test Escape/Cancel, switching keys, disabling hotkeys, and sleep while a short-release timeout is pending: no stale result should restart or lock recording.
6. Choose Right Option and repeat. If the keyboard never reports Fn, use this alternative. Quit/relaunch to verify preference persistence; restore the preferred key afterward.
7. Enable **Use while other apps are focused**. Without Accessibility permission, verify the local-only status. If you choose to grant permission through the explicit settings button, return and **Recheck access**. Test Fn from a disposable text field in another app: the pill must not steal focus; verify hold, double press, third press, and Copy. Check revocation and loss of local-only focus cancel active gestures, and the menu-bar Stop/Cancel remain available.
8. Confirm no real capture, insertion, submission or system-shortcut reconfiguration occurs in this preview. Approve the native hardware behavior before connecting the gesture controller to actual recording.

Pass: physical gestures match the requested semantics, lock state remains obvious, release/stop/cancel never resurrects a session, preferences persist, and scope/permission limitations are explicit. Record failures with key, keyboard model, timing, scope and current macOS Fn assignment; fix within this issue before production integration.

## Review fixes (2026-09-24)

- **Stale "Copied" badge.** Copying Cleaned and then switching to Original (or the reverse) still showed "Copied", even though the text on screen was not on the clipboard. The model now records which variant was copied (`CopyStatus` in `PrototypeCore`), and the badge appears only beside that variant. Regression check: `testCopiedStatusFollowsTheCopiedVariantOnly`.
- **Build failure on Command Line Tools–only Macs.** With Swift 6.4 and the macOS 27 SDK, SwiftUI's `@State` is a macro whose plugin (`SwiftUIMacros`) does not ship with Command Line Tools. Both `scripts/dev/preview-macos.sh` and the E2E failed to compile `PaletteView`. The palette's per-open state now uses a `@StateObject`. Re-ran `scripts/e2e/lf-163-macos-prototype.sh`: it passed with 7 session and 8 hotkey check groups. The cargo gate passed.
