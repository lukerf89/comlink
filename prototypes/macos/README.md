# Comlink native design preview · LF-163

A standalone macOS 14+ SwiftUI/AppKit **interaction prototype**, combining the quiet charcoal capsule with an explicitly opened command palette. No external packages; Command Line Tools with Swift 5.9+ are sufficient.

```bash
scripts/dev/preview-macos.sh
```

This builds and opens `prototypes/macos/.build/Comlink Preview.app`. Use the waveform in the macOS menu bar to reopen Preview controls, start/stop a sample, open the palette, or quit. The preview starts with a separate scenario controller; that controller is a design-review tool, not proposed everyday UI.

- Choose a scenario and mode, then **Start preview**. The controller closes and only the pill appears. Press its square to stop. After a simulated processing delay, a compact result appears without opening the palette.
- The menu-bar **Command palette…** action opens the secondary surface. Type to filter, use ↑/↓ and Return, or click an action. Escape goes back from a detail screen, then closes the palette.
- Choose Clean, Memo, Code, or Email before starting a sample. The selected mode is recorded on the sample. Output is intentionally a fixed fixture, not a Swift reimplementation of Rust cleanup.
- Switch **Cleaned / Original** in a result or palette review. **Copy** writes the displayed version to the real clipboard only when explicitly clicked. **Try Insert…** demonstrates failure and Copy fallback; it never pastes, types, or submits anything.
- Microphone permission and missing model fail before simulated capture. Their recovery buttons explicitly simulate a repaired environment. No-speech fails after Stop. Cancel discards the sample, including pending processing results.
- The app follows system light/dark appearance; the controller also offers Light/Dark overrides for review without changing macOS settings. The pill stays charcoal with high-contrast controls. Reduced Motion freezes the illustrative waveform; elapsed time remains available. Native controls and symbol-only buttons have accessibility labels.

## Recording hotkeys

Open the gear in Preview controls, **Hotkey settings…** in the menu bar, the palette's **Hotkey settings** action, or `⌘,` while the preview is focused.

- **Fn / Globe is the default.** Hold to record; release to finish. Double-press within 350 ms to lock the same recording on; the next Fn press stops it. A very short single tap waits until the double-press window expires before finishing.
- The pill shows a lock and **Locked · Fn to stop**. Stop and Cancel remain available.
- Choose Right Option instead, change the double-press window (250 / 350 / 500 ms), or disable the hotkey. Preferences save in this preview app's standard macOS preferences, independently of Rust CLI config. Changing settings cancels active hotkey recording.
- **Use while other apps are focused** is opt-in and needs macOS Accessibility access. The app opens the appropriate settings pane only when asked; it never grants itself permission. After granting access, use **Recheck access**. Without that access, input remains local to the focused preview.
- Fn/Globe may already trigger Emoji, an input-source switch, or Apple's double-Fn Dictation action. Settings explain how to avoid those conflicts; this app does not change or suppress system shortcuts. Third-party Fn bindings also require manual conflict checking.
- The **Preview hold / Preview double-press** buttons exercise the same gesture state machine without synthesizing keyboard events. For a locked sample, reopen settings and use **Preview next press** to finish it.

Native input adapters observe modifier changes and key-down event types, never typed characters. Combining a held recording key with another key cancels hold-to-talk; ordinary typing does not end an already locked recording. Sleep, settings changes, and loss of local-only focus cancel active hotkey gestures. Real recording/ASR remains simulated.

## Boundaries

No microphone, ASR, file reading, history database, Rust CLI config access, network, focus restoration, or insertion. All transcripts are synthetic and held in memory; no transcript logging or retention occurs. `⌘K` (palette) and `⌘,` (hotkey settings) are app-local bindings. Fn uses the selected local/cross-app scope; the earlier illustrative `⌥Space` binding was removed. Escape is local to a focused preview surface; use the cancel button or menu while another app is focused. A nonactivating panel lets the pill appear without taking keyboard focus, but real cross-app focus restoration still needs native integration tests.

The palette's file/history/vocabulary screens explain future behavior and provide a sample route where appropriate. They do not imply working file import, retained-history search, or vocabulary editing.

## Validation

```bash
scripts/e2e/lf-163-macos-prototype.sh
```

Runs fourteen dependency-free Swift check groups (six original + eight hotkey groups), builds the native executable and `.app`, validates its plist and startup JSON/stderr, and checks an isolated CLI config directory remains empty. A startup smoke check reads preview preferences but does not write them; persistence tests use a disposable suite. These are executable assertions (debug build), rather than XCTest, so they also work with Command Line Tools installations that lack XCTest. The script does not automate UI interaction; use the [native checklist](../../docs/validation/lf-163.md).

See [interaction decisions and integration proposal](../../docs/design/lf-163-macos-hybrid.md). Luke approved the visual preview and requested Fn hotkeys on 2026-09-24. Physical Fn/cross-app testing and production recording integration still require the [hotkey follow-up checklist](../../docs/validation/lf-163-hotkeys.md); no production phase advances automatically.
