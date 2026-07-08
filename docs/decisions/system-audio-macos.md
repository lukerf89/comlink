# Decision Record: macOS System Audio Capture

Date: 2026-07-08
Phase: 8 - Zoom/Teams System Audio Spike
Linear: LF-51

Status: **Accepted** — human gate decisions recorded below (2026-07-08).

## Decision

For the first online-meeting capture build, Comlink should require a documented
local virtual-audio dependency: **BlackHole 2ch (documented default)**. Detection
also accepts BlackHole 16ch when a user already runs it, but 2ch is the
recommended install and the configuration Phase 9 documents and validates first.
**Microsoft Teams is the first app validated on live hardware**, with Zoom
following. Phase 8 adds only diagnostics and documentation. It does not add
production capture.

Recommended Phase 9 shape:

- Capture `user_mic` from the selected microphone device.
- Capture `system_audio` from a BlackHole input device that receives Zoom,
  Teams, or default system output through a user-configured Multi-Output Device
  or Aggregate Device.
- If the implementation cannot keep the two streams separate, label the result
  `mixed` rather than pretending diarization or speaker identity exists.
- Keep capture local through FFmpeg/AVFoundation and local ASR.

## Why

BlackHole is the most practical v0 dependency because it exposes system audio as
a normal Core Audio input device that Comlink can detect and, in Phase 9, capture
with the same FFmpeg/AVFoundation path already used by `record` and `meet`. Its
upstream project describes it as a macOS virtual audio loopback driver for
routing audio between applications.

This keeps Comlink local-first and CLI-compatible. It avoids adding a signed
Swift/AppKit capture surface, avoids silent cloud fallback, and lets doctor
provide actionable setup diagnostics before any live meeting capture is shipped.

## Options Surveyed

### BlackHole virtual audio driver

BlackHole creates a virtual input/output device that lets applications route
audio to other applications. Comlink can detect it by listing AVFoundation audio
inputs through FFmpeg and looking for `BlackHole 2ch`, `BlackHole 16ch`, or a
configured `COMLINK_SYSTEM_AUDIO_DEVICE`.

Pros:

- Works with the current local CLI architecture and existing FFmpeg dependency.
- Does not require Comlink to use private APIs or screen-capture APIs.
- Can support mic-only, system-only, and mic-plus-system by selecting separate
  devices or an explicit mixed/aggregate device.
- Failure mode is explainable: the device is either visible as an audio input or
  it is not.

Cons:

- Requires a local install and user audio routing setup.
- Multi-Output/Aggregate Device setup must be documented clearly.
- Users must understand that meeting audio is being routed locally.
- GPL licensing means Comlink should depend on user installation, not bundle
  BlackHole, unless licensing is reviewed.

Sources:

- BlackHole upstream: https://github.com/ExistentialAudio/BlackHole
- BlackHole site: https://existential.audio/blackhole/

### Core Audio process taps

Apple documents Core Audio taps for capturing outgoing audio from a process or
group of processes. Community sample code around `AudioHardwareCreateProcessTap`
points to macOS 14.4 as the practical baseline for broad app/system audio
capture and notes that the permission prompt is tied to system audio recording.

Pros:

- No virtual audio driver if implemented successfully.
- Can theoretically capture a process or process group.
- Better long-term direction for an app-bundled Comlink surface.

Cons:

- macOS-version constrained, with macOS 14.4 as the practical target for this
  spike.
- Requires TCC system audio recording consent. There is no clean CLI-only
  permission story comparable to normal executable detection.
- Implementation is significantly more complex than the current FFmpeg adapter.
- A robust implementation likely needs a signed/bundled macOS helper so
  permissions are stable across builds and terminal sessions.

Sources:

- Apple Core Audio taps sample: https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps
- Apple AudioHardwareTap docs: https://developer.apple.com/documentation/coreaudio/audiohardwaretap
- AudioCap reference sample: https://github.com/insidegui/AudioCap

### ScreenCaptureKit

Apple positions ScreenCaptureKit as high-performance screen and audio capture.
Its WWDC material says it can capture screen content and associated audio, and
that capture requires user consent stored in Screen Recording privacy settings.

Pros:

- Official Apple framework.
- Handles audio and video together.
- Good fit for a GUI screen recorder or screen-sharing app.

Cons:

- Permission model is Screen Recording/TCC, not a simple CLI dependency.
- More video/screen-oriented than this phase's audio-only local transcript goal.
- Requires a macOS framework integration path outside the existing Rust
  subprocess capture adapter.
- Not ideal for a Phase 9 CLI-only v0.

Sources:

- ScreenCaptureKit docs: https://developer.apple.com/documentation/screencapturekit/
- WWDC22 ScreenCaptureKit session: https://developer.apple.com/videos/play/wwdc2022/10156/

### Aggregate and Multi-Output Devices

Aggregate Devices and Multi-Output Devices are useful routing configurations,
not a complete capture dependency by themselves. In the recommended path, they
are setup instructions that pair the user's speakers/headphones with BlackHole
so the user can still hear the meeting while Comlink captures the virtual input.

Decision: use as setup guidance with BlackHole, not as the dependency being
detected.

### Zoom local recording

Zoom supports computer recordings in the desktop app on macOS, and the resulting
files are saved under the user's local Zoom recording location. Account or host
settings can enable or disable the feature.

Pros:

- Official Zoom path.
- Can produce local audio files after the meeting.

Cons:

- Not live capture for Comlink.
- Depends on meeting role, host/account policy, and user action in Zoom.
- Produces post-meeting artifacts rather than Comlink-owned local transcript
  segments.
- Does not generalize to Teams.

Sources:

- Zoom managing computer recordings: https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0063423
- Zoom enabling computer recordings: https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0063640

### Microsoft Teams recording

Teams meeting recordings capture audio, video, and screen sharing, notify
participants, and are stored in Microsoft 365, typically OneDrive or SharePoint,
subject to Teams policy.

Pros:

- Official Teams recording path.
- Handles compliance/participant notification inside Teams.

Cons:

- Not local-first.
- Depends on tenant policy, organizer permissions, and Microsoft 365 storage.
- Does not provide Comlink-owned live local capture.
- Cannot be treated as an offline local dependency.

Sources:

- Teams recording user docs: https://support.microsoft.com/en-us/teams/meetings/start-stop-and-find-meeting-recordings-in-microsoft-teams
- Teams recording policy docs: https://learn.microsoft.com/en-us/microsoftteams/meeting-recording

## Permission Behavior

BlackHole path:

- Comlink sees BlackHole as an audio input.
- The meeting app routes audio to normal system output; the user configures that
  output to include BlackHole.
- Comlink still needs normal microphone permission when capturing `user_mic`.
- System audio routing itself is a local device setup issue, not a cloud or
  Zoom/Teams API permission.

Core Audio tap path:

- Requires macOS system audio recording consent.
- Needs an app identity that macOS can persist in TCC.
- This is not a good fit for a bare CLI spike.

ScreenCaptureKit path:

- Requires Screen Recording consent.
- Better suited to a bundled app that intentionally captures screen/audio.

Zoom:

- Local Zoom recording is a Zoom feature controlled by Zoom account, group, host,
  and app settings.
- Comlink should not depend on Zoom local recording for live local transcript
  capture.

Teams:

- Teams recordings are Microsoft 365 convenience recordings controlled by policy
  and stored in OneDrive or SharePoint.
- Comlink should not depend on Teams recording for local/offline capture.

## Source Metadata Prototype

Phase 9 should preserve source intent in metadata without claiming diarization:

```json
{
  "source": {
    "mode": "mic-plus-system",
    "streams": [
      {
        "label": "user_mic",
        "device": "MacBook Pro Microphone"
      },
      {
        "label": "system_audio",
        "device": "BlackHole 2ch"
      }
    ]
  }
}
```

Recommended labels:

- `user_mic`: local microphone capture.
- `system_audio`: routed meeting/system output capture.
- `mixed`: a combined stream where Comlink cannot reliably separate mic and
  system audio.

Recommended modes:

- `mic-only`: current in-person meeting behavior.
- `system-only`: BlackHole or other approved system-audio input only.
- `mic-plus-system`: two local inputs captured and preserved as separate source
  metadata where technically possible.

## Doctor Diagnostic

Phase 8 adds an additive `system_audio` object to `comlink.doctor.v1` and a
non-required `system-audio` check.

The diagnostic reports:

- Whether the selected Phase 8 dependency is available.
- The chosen strategy: `blackhole-virtual-audio-device`.
- The detected BlackHole device name, when present.
- Native Core Audio tap OS support as informational only.
- Actionable remediation when system-audio capture is unavailable.
- Prototype source metadata labels: `user_mic`, `system_audio`, and `mixed`.

This diagnostic must not make `doctor` fail in Phase 8 because production
system-audio capture is not yet shipped.

## Rejected Alternatives

- Shipping Phase 9 directly on Core Audio taps: rejected for v0 because the
  permission and packaging story is not clear enough for the CLI.
- Shipping Phase 9 directly on ScreenCaptureKit: rejected for v0 because it is
  screen-capture oriented and needs Screen Recording consent.
- Using Zoom local recordings as the integration: rejected because it is not
  live, not general across apps, and depends on Zoom policy.
- Using Teams recordings as the integration: rejected because it is cloud-backed
  Microsoft 365 recording, not local/offline Comlink capture.
- Bundling a virtual audio driver: rejected for now pending license, installer,
  update, and user-trust review.

## Human Gate Decisions (2026-07-08)

Resolved at the Phase 8 manual gate:

1. **Requiring a local virtual-audio dependency is acceptable.** Phase 9 may
   depend on the user installing BlackHole.
2. **BlackHole 2ch is the documented default.** Meeting audio from Zoom/Teams is
   mono/stereo and Comlink downmixes to 16 kHz mono for local ASR, so the extra
   channels in 16ch add routing/CPU overhead with no benefit for this use case.
   Detection remains permissive (2ch or 16ch, or `COMLINK_SYSTEM_AUDIO_DEVICE`)
   so a user already on 16ch is not blocked, but setup docs and validation target
   2ch.
3. **Microsoft Teams is validated first** on live hardware; Zoom follows.

Still open, to be answered before/within Phase 9 (not blocking this spike):

4. Is a signed macOS helper/app acceptable later if the project wants to move
   from BlackHole to native Core Audio taps?
5. What user-facing consent language should Comlink show before online meeting
   capture?
