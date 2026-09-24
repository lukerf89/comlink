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

## Boundaries

No microphone, ASR, file reading, history database, config access, network, global shortcut registration, focus restoration, or insertion. All transcripts are synthetic and held in memory; no transcript logging or retention occurs. `⌥Space` (toggle start/stop) and `⌘K` (palette) are app-local demonstration shortcuts, not system-wide bindings. Escape is local to a focused preview surface; use the cancel button or menu while another app is focused. A nonactivating panel lets the pill appear without taking keyboard focus, but real cross-app focus restoration still needs native integration tests.

The palette's file/history/vocabulary screens explain future behavior and provide a sample route where appropriate. They do not imply working file import, retained-history search, or vocabulary editing.

## Validation

```bash
scripts/e2e/lf-163-macos-prototype.sh
```

Runs six dependency-free Swift check groups, builds the native executable and `.app`, validates its plist, and checks an isolated config directory remains empty. These are executable assertions (debug build), rather than XCTest, so they also work with Command Line Tools installations that lack XCTest. The script does not automate UI interaction; use the [native checklist](../../docs/validation/lf-163.md).

See [interaction decisions and integration proposal](../../docs/design/lf-163-macos-hybrid.md). Luke's design review remains required before treating proposed interactions as accepted or starting a production phase.
