# LF-163 implementation plan

Scope: a native, executable design prototype and implementation proposal. No production recording, global hotkey, insertion, CLI changes, or phase advancement.

1. Build a dependency-free Swift/AppKit + SwiftUI preview with a menu-bar entry, nonactivating charcoal recording pill, compact result card, and separately invoked keyboard command palette.
2. Test the simulated state machine before wiring views: idle/listening/processing/completed/canceled, permission/model/no-speech/insertion failure, and stale completion rejection.
3. Make all synthetic scenarios reachable from a separate preview controller; demonstrate mode selection and original/cleaned review. Actual clipboard writes only on explicit Copy.
4. Document proposed toggle shortcut, focus restoration and review-first delivery as pending Luke's approval, plus a small Rust-core integration slice mapped to current phase gates.
5. Launch the native app, inspect and interact with its surfaces, capture synthetic-only previews, run Swift tests and the four required Rust gates, and open a PR for design review.

The existing plan status table stops at Phase 4, while code and validation reports extend through later phases. Do not infer human approval from those files; reconcile manual acceptance before scheduling production integration.

## Follow-up: Fn hotkeys (2026-09-24)

Luke approved the visual preview and requested hotkey settings: Fn by default,
double Fn locks recording until another Fn press. Interpret single Fn as
hold-to-talk. Add persisted preview-only settings, a tested gesture state
machine, native modifier-event adapters (local plus opt-in accessibility-gated
cross-app observation), visible locked feedback, and an interactive gesture test.
Keep ASR simulated and preserve the production Rust CLI. Document macOS Fn/Globe
conflicts without changing system settings. Re-run native checks and the Rust
gate, launch/inspect settings and locked states, and update PR #16.
