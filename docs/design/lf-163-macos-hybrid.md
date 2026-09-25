# Comlink: quiet capture, commands on demand

LF-163 design proposal, updated 2026-09-24. **Luke approved the visual direction and requested Fn hotkeys.** This runnable native prototype implements an interaction proposal, not production dictation or approval to advance a phase.

## Everyday flow

The menu-bar waveform opens a correctly anchored native menu. Start dictation presents a compact charcoal capsule: microphone, white waveform, elapsed time, square Stop, and quiet Cancel. The panel does not activate the app when shown. Stopping replaces the waveform with a processing label. No live transcript is shown or promised.

Completion opens a compact review card with cleaned text selected, the original available, and explicit Copy. The palette is absent throughout this flow. Cancel shows a short acknowledgment and discards the sample. Permission/model failures happen before capture; no-speech occurs after processing. Insertion failure keeps the transcript and offers Copy.

The optional palette is opened deliberately. It supports command search, wrapping arrow-key selection, Return, Escape, mode selection, and transcript review. File transcription, retained history, and vocabulary/snippets have clearly labeled future-state screens. No large history window is introduced.

## Proposed decisions for review

| Decision | Prototype / recommendation | Production acceptance still needed |
| --- | --- | --- |
| Recording gesture | Hold Fn to record; release to finish. Double Fn locks until the next Fn press, per Luke’s follow-up. | Physical hardware and cross-app validation; live capture remains simulated. |
| Dictation shortcut | Fn / Globe default; Right Option alternative; configurable timing and opt-in cross-app observation with Accessibility access. Menu bar remains available. | Manual macOS and third-party Fn conflict checks; the preview never suppresses or changes existing system bindings. |
| Palette shortcut | Illustrative `⌘K` while preview is active; menu-bar action works independently. | Select a distinct configurable system-wide binding if desired. |
| Delivery | Review first; Copy is the primary action. No automatic insertion. | Luke approves review policy; manual paste is sufficient for the first slice. |
| Insert | Explicit action only, with Copy fallback. Preview deliberately simulates failure. | Gate behind capability checks and separate opt-in. Never synthesize Return or any submit/send event. |
| Focus | Show capture in a nonactivating panel; only explicit palette/review interaction takes focus. | Snapshot the previously focused app/element before activation. Restore only the still-valid intended target on an explicit Insert. Never fall back to whichever field happens to be focused. |
| Missing or changed target | Keep transcript visible and explain Copy fallback. | Test closed windows, changed selection, secure fields, permission denial, and target app termination. |
| Modes | Clean / Memo / Code / Email map to `clean`, `memo`, `coding-prompt`, `email-reply`. | Use Rust's mode registry; no separate Swift cleanup implementation. |
| Retention | Prototype holds synthetic text in memory only; there is no history persistence. | Real history honors metadata/transcript/audio settings, including retention disabled. |

The visible preview controller is test scaffolding. It is not onboarding or an everyday application window. Permission recovery buttons simulate success; production must recheck actual permission/model state. The insertion scenario exposes the same review-first result, then demonstrates failure when Try Insert is clicked.

## Smallest first production slice

After design review and reconciliation of the phase gates, ship **menu-bar manual dictation → pill → local processing → review → Copy**. Reuse the existing Rust core and schema v1. The follow-up implements native modifier hotkeys for the preview, including optional cross-app observation. Reuse that adapter after physical-key validation when integrating real recording; defer insertion, file/history/vocabulary GUI editing, and any system audio work. The palette may initially expose Start, mode selection, and review only; do not ship nonfunctional future commands.

1. Extract a cancellable session service around existing `src/record.rs`, `src/audio.rs`, `src/asr.rs`, `src/text.rs`, and `src/output.rs`. The current recording entry point is stdin/Enter-driven; it is not already a GUI-ready start/stop API. Define ownership, capture cleanup on cancel/quit, and session IDs before integrating a UI.
2. Choose and document a small bridge (Rust static library/C ABI or a versioned local process protocol). Expose start/stop/cancel/result without making Swift responsible for ASR or deterministic cleanup. Exercise contract tests across the chosen boundary. Keep CLI stdout/stderr/schema/exit codes unchanged.
3. Implement macOS microphone authorization and capture lifecycle through adapters; keep clipboard and later focus/shortcut/paste I/O behind adapters. Never log transcript payloads. On LLM failure use deterministic fallback; `--no-llm` stays supported. No silent cloud fallback.
4. Add a signed app bundle with permission usage text, lifecycle cleanup, menu-bar status, accessible controls, and result review. No microphone permission is requested by this design prototype.
5. Test real capture, cancel during capture/processing, missing model, no speech, clipboard failure, raw preservation, and retention off. Run Rust gates plus bridge tests and native manual validation before asking for production acceptance.

## Mapping to the existing plan

| Existing scope | Relationship to proposed app |
| --- | --- |
| Phases 1–4 | Reuse recording, normalization, ASR, text modes, config/storage/retention, and v1 output. Native lifecycle integration is new work, not permission to change these contracts. |
| Phase 5 | Optional local rewrite may be exposed later using existing fallback and context policy. Not required by the first app slice. |
| Phase 6 | Copy-first delivery, microphone/clipboard diagnostics, surface formatting, and manual work-surface tests apply directly. |
| Phases 7–9 | Meetings and system audio are unrelated to this minimal dictation prototype. |
| Phase 10 | Luke explicitly requested Fn hotkeys for this preview. Production capture integration and focused-field insertion still need scoped approval and native tests; no phase gate is advanced. |

The plan's summary still says to resume at Phase 4 manual review, while later code and validation documents exist through the supplemental Phase 10 real-audio suite. Those artifacts are not evidence that all human gates passed. Reconcile acceptance records with Luke before scheduling the production slice; this PR does not rewrite phase status.

## Accessibility and appearance

Use system fonts, native controls, semantic foreground colors, restrained system blue, regular material, and hairline borders. Light/dark appearance follows macOS; the hero capsule stays charcoal in both. Reduced Motion freezes the sample waveform. Named Stop, Cancel, Dismiss, Back, and Close controls are exposed to accessibility. Search is focused on palette open; arrow/Return navigation is independent of pointer hover. VoiceOver reading order and Full Keyboard Access require the human checks in the validation document.

## References

The two selected visual boards and original brief are attached to [LF-163](https://linear.app/harrow-software/issue/LF-163/design-minimal-macos-app-recording-pill-with-on-demand-command-palette). The prototype preserves concept 1's rounded charcoal capsule and concept 2's compact action palette. The pre-existing local `docs/design/macos-concepts/` exploration is left untouched and outside this change.

## Fn hotkey follow-up

The gesture resolver starts one sample on the first key-down. A long hold finishes on release; a short tap waits only until first-down + the selected double-press window. A second press within that interval locks the **same** sample without processing/restarting between taps. Releasing the second press leaves it locked; the next press stops. Processing ignores hotkey presses. Stop, Cancel, preference changes and sleep reset gesture/timer state.

Settings use an app-local UserDefaults adapter; input uses passive NSEvent local/global monitors behind a separate adapter. No keyboard event characters are read or logged. Global observation is opt-in and gated by `AXIsProcessTrusted`; without permission the app explains its local-only fallback. Fn+another key cancels a held sample, while ordinary typing during locked recording is allowed. The nonactivating pill gains a lock icon and accessible stop instruction.

Apple documents the access requirement for [global event monitors](https://developer.apple.com/documentation/appkit/nsevent/addglobalmonitorforevents(matching:handler:)) and the interaction between [Fn/Globe and double-Fn Dictation settings](https://support.apple.com/guide/mac-help/mh40584/mac). The preview gives conflict guidance and does not reconfigure macOS. See [follow-up validation](../validation/lf-163-hotkeys.md) for observed checks and the physical-key manual gate.
