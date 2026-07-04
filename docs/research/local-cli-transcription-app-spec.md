# Local CLI Transcription App Research & Build Spec

Date: 2026-07-03  
Scope: public research into Wispr Flow-style dictation/transcription products, open-source implementations, and local open-weight speech models. This document avoids private binary analysis; "reverse-engineering" here means inferring architecture from product docs, public repos, and observable feature behavior.

## Executive Summary

Modern AI dictation apps are not just "speech-to-text with a hotkey." The product magic comes from a layered pipeline:

```text
global trigger -> audio capture -> pre-roll/VAD/endpointing -> ASR -> cleanup/transforms -> context-aware formatting -> paste/copy/save
```

The best local CLI version should be built as a small command-line control plane around local engines:

- Default ASR: `whisper.cpp` for broad cross-platform local inference and easy packaging.
- Optional high-throughput ASR: `faster-whisper` for CUDA/GPU batch transcription.
- Optional low-latency ASR: NVIDIA Parakeet for English/EU languages where runtime packaging is acceptable.
- VAD: Silero VAD for fast silence detection and speech boundary handling.
- Optional diarization: pyannote/WhisperX-style pipeline for files and meetings, not MVP dictation.
- Optional rewrite layer: local Ollama or `llama.cpp` OpenAI-compatible endpoint, with deterministic cleanup always available without an LLM.

The recommended MVP is a local-first CLI named `comlink`:

```bash
comlink record --paste
comlink transcribe meeting.m4a --format md
comlink listen --hotkey alt-space
comlink models install whisper-small.en
comlink vocab add "supabase" "Supabase"
comlink modes add prompt --instruction "Turn rough speech into a concise coding prompt."
```

## What Wispr Flow-Like Apps Actually Do

### Wispr Flow

Wispr Flow positions itself as system-wide voice dictation: press a hotkey, speak naturally, and text appears in any text field. Its docs describe real-time transcription, AI commands such as "make this more professional," and vocabulary learning over time. The desktop "Hub" includes history, dictionary, snippets, notes, and usage stats.

Important architecture clues:

- It is cloud-only for transcription. Wispr's data controls page says transcription always occurs in the cloud.
- It uses context: app name, surrounding textbox content, and optional active-window text can influence capitalization, punctuation, command mode, and accuracy.
- Personalization includes dictionary, snippets, styles, team dictionaries/snippets, and usage dashboards.
- Developer-focused features include code syntax awareness, CLI command formatting, camelCase/snake_case handling, jargon, and file tagging in Cursor/Windsurf.
- Privacy mode changes retention/training policy, not local processing. Cloud Sync controls whether Wispr stores audio/transcripts/history.

Sources: [What is Flow?](https://docs.wisprflow.ai/articles/2772472373-what-is-flow), [Wispr Flow features](https://wisprflow.ai/features), [Wispr Flow data controls](https://wisprflow.ai/data-controls), [Wispr Flow business/security positioning](https://wisprflow.ai/business).

### Superwhisper

Superwhisper is the cleanest product analogue for a local-first implementation. Its docs expose the mental model directly: "modes" combine voice processing, optional AI processing, language/model choices, app/website auto-activation, system audio recording, and speaker identification.

Important architecture clues:

- Voice processing is separate from AI processing. Audio first becomes text, then an LLM may reshape it.
- Modes can choose language, local/cloud voice model, local/cloud LLM model, and custom instructions.
- Auto-activation rules switch modes by active app or website.
- Local voice models use Whisper via `whisper.cpp`; Superwhisper also documents local NVIDIA Parakeet options.
- Meeting-oriented capture can record system audio and identify speakers.
- Some modes intentionally skip AI processing for speed.

Sources: [Superwhisper introduction](https://superwhisper.com/docs/get-started/introduction), [Superwhisper modes](https://superwhisper.com/docs/modes/modes), [Superwhisper voice models](https://superwhisper.com/docs/models/voice), [Superwhisper language models](https://superwhisper.com/docs/models/language), [Superwhisper models](https://superwhisper.com/models).

### Aqua Voice

Aqua emphasizes real-time text refinement, developer-language accuracy, custom dictionary, writing style rules, and using screen context as a dictionary. It markets its proprietary Avalon model and a real-time editing experience rather than raw transcription.

Important architecture clues:

- Real-time UX is a major differentiator: words/refinements appear as the user speaks.
- Developer dictation requires syntax, library, framework, and code-symbol awareness.
- Custom dictionary and writing style rules are first-class.
- Context is part of accuracy, not only formatting.
- Privacy posture is product-specific and should not be assumed local just because UX is native.

Sources: [Aqua Voice homepage](https://aquavoice.com/), [Aqua use cases](https://aquavoice.com/use-cases), [Aqua privacy policy](https://aquavoice.com/info/privacy).

### Granola

Granola is meeting-first rather than dictation-first, but its design strongly informs a local CLI "meeting mode." It runs without a meeting bot, captures on-device system audio and microphone, transcribes when the user explicitly starts a note/meeting, shows a live transcript panel, and stores transcript/notes rather than audio.

Important architecture clues:

- Meeting transcription has a different lifecycle from short dictation.
- Explicit start/stop and visible recording state are essential.
- Dual-source capture is valuable: system audio usually represents other speakers, mic audio represents the user.
- Live transcript chunks should be copyable/searchable/deletable.
- Auto-stop heuristics use audio inactivity, calendar end time, call app state, and sleep events.
- Notes and transcripts may be retained even when audio is not.

Sources: [Granola transcription docs](https://docs.granola.ai/help-center/taking-notes/transcription), [Granola security](https://www.granola.ai/security), [Granola security/privacy FAQ](https://docs.granola.ai/help-center/consent-security-privacy/security-privacy-data-faqs).

### Local-First and Open-Source Apps

Public local-first implementations show how the above product layer maps into code.

OpenWhispr is an Electron/React cross-platform app with native helper binaries for hotkeys, fast paste, microphone/system-audio monitoring, and text monitoring. Its stack includes Electron, better-sqlite3, `whisper.cpp`, sherpa-onnx, `llama.cpp`, local diarization models, and cross-platform native helpers.

VoiceInk is a native macOS app with modules for recording, paste delivery, active-window/browser URL context, selected text, modes, local/cloud transcription providers, local CLI LLM integration, Ollama, history, dictionary, word replacement, shortcuts, and prewarm services.

MacParakeet is a native macOS reference for a sophisticated local architecture: shared microphone stream, dictation pipeline, meeting pipeline, file/URL pipeline, STT scheduler, local engine runtime, deterministic text processing, SQLite persistence, CLI commands, model management, vocabulary, snippets, transforms, and explicit scheduling between interactive and background jobs.

MacWhisper and Aiko show the simpler local transcription end: strong file/audio transcription, local Whisper models, export formats, privacy by on-device inference, and weaker real-time dictation semantics.

Sources: [OpenWhispr repo](https://github.com/OpenWhispr/openwhispr), [OpenWhispr local Whisper setup](https://github.com/OpenWhispr/openwhispr/blob/main/LOCAL_WHISPER_SETUP.md), [VoiceInk repo](https://github.com/beingpax/VoiceInk), [VoiceInk site](https://tryvoiceink.com/), [MacParakeet repo](https://github.com/moona3k/macparakeet), [MacWhisper](https://www.macwhisper.com/), [Aiko](https://sindresorhus.com/aiko).

## Competitive Feature Matrix

| Product | Primary job | Local/offline | System-wide dictation | Live partials | Context awareness | Modes/styles | Dictionary/snippets | Meeting capture | Diarization | Notes/history |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Wispr Flow | Polished dictation | No, cloud ASR | Yes | Yes | App/textbox/window | Styles + AI commands | Yes | Limited | Not core | Hub, notes, stats |
| Superwhisper | Local/cloud dictation | Yes, selectable | Yes | Some modes | App/site/selected context | Strong modes | Yes | Yes | Yes | History |
| Aqua | Real-time polished dictation | Product-specific/cloud-oriented | Yes | Yes | Screen/app context | Writing rules | Yes | Not core | No | Private history |
| Granola | Meeting notes | Hybrid local capture/cloud processing | No | Yes | Calendar/meeting note context | Templates/recipes | Customization | Yes | Speaker identification | Notes/transcripts |
| MacWhisper | Local transcription | Yes | Mac direct version supports dictation | Mostly file/batch | Limited | AI provider integrations | Limited | App audio | Speaker recognition | Transcripts |
| Aiko | Local file transcription | Yes | No | No | No | No | Word replacement | Record/import | No | Exports |
| VoiceInk | Local Mac dictation | Yes | Yes | Yes/near-real-time | Screen/app/URL | Strong modes | Yes | Not primary | Not primary | History |
| OpenWhispr | Local-first desktop assistant | Yes | Yes | Yes | Text monitoring | AI actions | Yes | Yes | Yes | Notes/search |
| MacParakeet | Local Mac voice app | Yes | Yes | Yes | App/prompt context | Transforms/profiles | Yes | Yes | Pipeline-level | SQLite library |

## Reverse-Engineered Architecture Pattern

### 1. Trigger Layer

Responsibilities:

- Global hotkeys and push-to-talk.
- Press-to-start/press-to-stop and hold-to-record gestures.
- Separate triggers for dictation, agent command, meeting, and transform-selected-text.
- Debounce and conflict detection.
- Visible status: recording, transcribing, pasted, failed.

CLI implication:

- MVP can start with foreground commands (`comlink record`, press Enter to stop).
- System-wide dictation requires a daemon (`comlink listen`) plus OS-specific helpers.
- Do not put hotkey handling directly inside the ASR engine; keep it as an input adapter.

### 2. Audio Capture Layer

Responsibilities:

- Microphone selection, permissions, level metering.
- Format normalization to 16 kHz mono PCM/WAV for most ASR engines.
- Optional pre-roll so the first syllable is not clipped.
- VAD/silence endpointing.
- File/video conversion through FFmpeg.
- Meeting dual-source capture when supported: mic + system audio.

Observed patterns:

- Local apps normalize everything before inference.
- Meeting mode is a separate pipeline, not just "long dictation."
- Production apps include minimum-duration guards to avoid hallucinations on empty audio.
- Pre-roll helps dictation feel instant but must be RAM-only and bounded.

### 3. Speech Boundary / VAD Layer

Responsibilities:

- Detect speech start/stop.
- Trim leading/trailing silence.
- Split long files into speech chunks.
- Prevent empty/silent audio from reaching Whisper-like models.

Recommended:

- Silero VAD as default: small, fast, permissively licensed.
- WebRTC VAD as emergency fallback for minimal builds.
- VAD should output timestamped speech regions so both ASR and history can show what was actually processed.

Source: [Silero VAD](https://github.com/snakers4/silero-vad).

### 4. ASR Engine Layer

Responsibilities:

- Transcribe normalized audio into raw text plus optional timestamps/confidence.
- Provide model selection by speed/quality/language.
- Hide engine-specific differences behind a common result schema.

Recommended engine interface:

```json
{
  "text": "raw transcript",
  "language": "en",
  "duration_ms": 3140,
  "segments": [
    {
      "start_ms": 0,
      "end_ms": 3140,
      "text": "raw transcript",
      "words": [
        { "start_ms": 120, "end_ms": 300, "text": "raw", "confidence": 0.91 }
      ]
    }
  ],
  "engine": "whisper.cpp",
  "model": "small.en"
}
```

### 5. Text Processing Layer

Responsibilities:

- Safe deterministic cleanup: remove "um/uh", trim spaces, normalize punctuation spacing.
- Apply vocabulary and custom replacements.
- Expand snippets.
- Extract terminal actions such as "press enter."
- Optional local LLM rewrite/style transform.
- Preserve exact/raw transcript when requested.

Key product lesson:

Use deterministic cleanup before LLM rewriting. It is fast, testable, offline, and predictable. LLM modes are powerful, but users need a raw/clean mode that never invents content.

### 6. Context Layer

Responsibilities:

- Active app/window name.
- Selected text.
- Clipboard.
- Current shell working directory, git branch, and editor/file hints for CLI use.
- Optional screen/textbox context with explicit opt-in.

CLI implication:

- In normal CLI mode, context is explicit: flags, stdin, current directory.
- In daemon mode, active app name can be enabled by default because it is useful for mode selection and relatively low sensitivity.
- Default should avoid screenshots. Clipboard, surrounding text, textbox content, and selected text should be opt-in and visible in logs/history.

### 7. Delivery Layer

Responsibilities:

- Print to stdout.
- Copy to clipboard.
- Paste into active app.
- Save transcript/history.
- Emit JSON for scripts.
- Serve API/MCP for agents.

Observed pattern:

Fast paste is one of the hardest cross-platform details. Open-source desktop apps include native helpers for Windows `SendInput`, macOS paste/accessibility behavior, and Linux X11/Wayland/uinput/portal differences. A CLI should treat "paste" as a pluggable adapter, not as a universal shortcut.

### 8. Storage Layer

Responsibilities:

- Model cache.
- Config.
- History database.
- Optional retained audio.
- Vocabulary/snippets/modes.
- Logs.

Recommended default:

- Local-only SQLite.
- Audio retention off by default for short dictation.
- Retain session metadata by default.
- Retain transcript text only if user enables transcript history or a command requests `--save`.
- Models under XDG/macOS/Windows cache paths.

## Open-Weight Model & Runtime Choices

### Whisper

Whisper remains the safest baseline because it is multilingual, robust to accents/noise/technical language, widely ported, and MIT licensed. The official repo documents model sizes from `tiny` to `large` and `turbo`, with tradeoffs in VRAM and speed. Whisper processes audio in sliding 30-second windows, which is good for file transcription but awkward for true token-by-token live dictation.

Use cases:

- Default broad-coverage dictation.
- File transcription.
- Multilingual transcription and translation.
- CPU-friendly local mode via smaller GGML/GGUF models.

Sources: [OpenAI Whisper repo](https://github.com/openai/whisper), [Whisper model card](https://github.com/openai/whisper/blob/main/model-card.md).

### whisper.cpp

`whisper.cpp` is the best MVP runtime for a CLI because it is C/C++, supports macOS, iOS, Android, Linux, FreeBSD, WebAssembly, Windows, Raspberry Pi, and Docker, and can be distributed as a small binary plus model files. It also has microphone streaming examples, but those should be treated as chunked pseudo-streaming rather than a perfect real-time architecture.

Use cases:

- MVP local CLI.
- Portable CPU inference.
- Packaged desktop helpers.
- Simple subprocess integration.

Source: [whisper.cpp](https://github.com/ggml-org/whisper.cpp).

### faster-whisper

`faster-whisper` reimplements Whisper with CTranslate2 and can be much faster with lower memory use, especially on GPU and with quantization. It also supports batched transcription, making it attractive for file queues, meeting archives, and server/batch mode.

Use cases:

- GPU-accelerated batch transcription.
- Large audio/video queues.
- WhisperX-style pipeline.

Source: [faster-whisper](https://github.com/SYSTRAN/faster-whisper).

### WhisperX

WhisperX layers VAD, batched `faster-whisper`, forced alignment, word-level timestamps, and pyannote speaker diarization. It is overkill for short dictation but excellent for files, subtitles, interviews, meetings, and any workflow where "who said what when" matters.

Use cases:

- `comlink transcribe --diarize`.
- SRT/VTT export.
- Interview and meeting file transcription.

Source: [WhisperX](https://github.com/m-bain/whisperX).

### NVIDIA Parakeet

Parakeet is compelling for low-latency local dictation. Current public model cards include:

- `nvidia/parakeet-tdt-0.6b-v3`: 600M multilingual ASR for 25 European languages, designed for high-throughput transcription.
- `nvidia/parakeet-unified-en-0.6b`: English ASR with offline and streaming inference in one model, minimum latency described as 160 ms, with punctuation/capitalization support.

Tradeoffs:

- Runtime packaging is less universal than `whisper.cpp`.
- Language coverage is narrower than Whisper.
- Licensing varies by model; verify before embedding in a commercial distribution.
- For macOS native apps, CoreML/ANE paths are attractive; for CLI cross-platform, Python/Transformers/NeMo or ONNX/sherpa-style paths may be easier but heavier.

Sources: [Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), [Parakeet Unified EN 0.6B](https://huggingface.co/nvidia/parakeet-unified-en-0.6b).

### NVIDIA Canary

Canary is a strong optional model family for multilingual transcription and speech translation. `nvidia/canary-1b-v2` is documented as ASR/AST across 25 European languages under CC-BY-4.0.

Use cases:

- Multilingual transcription/translation when Whisper is not desired.
- Accuracy experiments.

Source: [Canary 1B v2](https://huggingface.co/nvidia/canary-1b-v2).

### Diarization

Use diarization only when needed. It adds dependencies, runtime cost, and model access conditions.

Recommended:

- MVP: no diarization.
- Phase 2: lightweight meeting mode with explicit start/stop, visible recording state, mic capture, and transcript export. Use source labels when multiple audio sources are available, but do not require diarization.
- Phase 3: meeting polish with source-separated capture where supported, better chunk stitching, Markdown export, and optional note markers.
- Phase 4: WhisperX/pyannote for diarization, alignment, and file/meeting pro workflows. Mic/system-source separation often produces more useful labels than generic diarization alone.

Sources: [pyannote.audio](https://github.com/pyannote/pyannote-audio), [WhisperX](https://github.com/m-bain/whisperX).

### Local LLM Rewrite Layer

For local AI cleanup, support OpenAI-compatible local servers and Ollama:

- Ollama has a local REST API at `localhost:11434`.
- `llama.cpp` includes `llama-server`, an OpenAI-compatible local HTTP server.

The CLI should not require an LLM. LLM rewriting should be mode-specific and disabled by default for raw transcripts.

Sources: [Ollama API docs](https://docs.ollama.com/api/introduction), [llama.cpp server](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md).

## Product Spec: `comlink`, A Local CLI Transcription App

### Goals

1. Dictate short text locally from the terminal or a global hotkey.
2. Transcribe local audio/video files with open-weight models.
3. Produce clean, paste-ready text for coding prompts, emails, notes, and terminal commands.
4. Run without cloud services after initial model download.
5. Make privacy inspectable: clear network behavior, explicit retention, local logs.
6. Expose machine-readable output for scripts and agents.

### Non-Goals for MVP

- Mobile app.
- Cloud transcription.
- Team sync.
- Real-time collaborative notes.
- Perfect meeting assistant parity with Granola.
- Background screenshot analysis.
- Automatic training on user corrections.

### User Stories

- As a developer, I can speak a rough coding prompt and paste a cleaned version into Cursor, Claude Code, or a terminal.
- As a writer, I can dictate a paragraph and choose raw, clean, or rewrite modes.
- As a researcher, I can transcribe a downloaded interview and export Markdown/SRT/JSON.
- As a privacy-sensitive user, I can prove the app made no network requests during transcription.
- As a power user, I can script transcription in shell pipelines.

### Resolved Product Decisions

- MVP shape: terminal-first.
- Core implementation language: Rust.
- Default ASR path: `whisper.cpp` with `small.en` as the recommended default model.
- Platform priority: macOS first.
- History default: session metadata on, transcript text and audio retention off unless explicitly enabled.
- Context default: CLI/git context plus active app name; clipboard, selected text, screenshots, and textbox content require explicit opt-in.
- Delivery strategy: copy-first MVP, with active-app paste added through daemon adapters.
- Terminal/coding mode: Phase 1, because developer dictation is central to the product.
- Meeting mode: Phase 2 v0 with explicit start/stop, visible recording state, mic-first capture, and transcript export; diarization remains a later pro feature.

## CLI Surface

### Core Commands

```bash
# Record from microphone, stop on Enter, print transcript.
comlink record

# Record and copy/paste result.
comlink record --copy
comlink record --paste

# Push-to-talk daemon with global hotkey.
comlink listen --hotkey alt-space --mode clean

# Transcribe files.
comlink transcribe audio.wav
comlink transcribe video.mp4 --format json
comlink transcribe interview.m4a --format srt

# Transcribe stdin audio bytes or file list.
cat audio.wav | comlink transcribe -
find calls -name '*.mp3' | comlink batch --format md

# Model management.
comlink models list
comlink models install whisper-base.en
comlink models install whisper-small
comlink models select whisper-small.en

# Vocabulary/snippets.
comlink vocab add "aye pee eye" "API"
comlink vocab add "super base" "Supabase"
comlink snippets add "my signature" "Best,\nLuke"

# Modes.
comlink modes list
comlink modes add prompt --instruction "Convert rambling dictation into a concise coding prompt. Preserve filenames and code symbols exactly."
comlink record --mode prompt --paste

# History.
comlink history list
comlink history show <id>
comlink history prune --older-than 14d

# Diagnostics.
comlink doctor
comlink devices
comlink config show
comlink privacy audit
```

### Output Formats

- `text`: final text only.
- `json`: full schema with timings, engine, model, duration, mode, and processing steps.
- `md`: readable transcript with metadata header.
- `srt` / `vtt`: subtitles when timestamps exist.
- `segments`: line-delimited segments for streaming into other tools.

### Exit Codes

- `0`: success.
- `1`: general error.
- `2`: audio device/permission error.
- `3`: model missing or failed to load.
- `4`: no speech detected.
- `5`: paste/copy delivery failed after successful transcription.
- `6`: offline policy violation blocked network/model download.

## Configuration

Config path:

- macOS: `~/Library/Application Support/comlink/config.toml`
- Linux: `~/.config/comlink/config.toml`
- Windows: `%APPDATA%\comlink\config.toml`

Example:

```toml
[privacy]
offline = true
save_audio = false
save_session_metadata = true
save_transcripts = false
allow_active_app_context = true
allow_clipboard_context = false
allow_selected_text_context = false
allow_textbox_context = false

[audio]
device = "default"
sample_rate = 16000
channels = 1
pre_roll_ms = 450
min_speech_ms = 300
silence_stop_ms = 900
max_record_seconds = 120

[asr]
engine = "whisper.cpp"
model = "small.en"
language = "auto"
threads = 8
timestamps = true

[vad]
engine = "silero"
threshold = 0.5
min_speech_ms = 250
min_silence_ms = 600

[text]
mode = "clean"
remove_fillers = true
expand_snippets = true
apply_vocabulary = true
insertion_style = "sentence"

[llm]
enabled = false
provider = "ollama"
base_url = "http://localhost:11434"
model = "local-default"
```

## Data Model

Use SQLite for local history and settings that benefit from querying.

Tables:

- `sessions`: id, kind, started_at, duration_ms, engine, model, language, mode, text_raw, text_final, audio_path nullable, metadata_json.
- `segments`: session_id, index, start_ms, end_ms, speaker nullable, text, confidence nullable.
- `words`: segment_id, index, start_ms, end_ms, text, confidence nullable.
- `vocabulary`: id, phrase, replacement, enabled, created_at, updated_at.
- `snippets`: id, trigger, expansion, action nullable, enabled, use_count.
- `modes`: id, name, kind, deterministic_settings_json, llm_instruction nullable, context_policy_json.
- `model_registry`: id, engine, model, path, license, installed_at, bytes.

Storage paths:

- Models: OS cache dir, e.g. `~/Library/Caches/comlink/models`.
- Temp audio: OS temp dir, removed on completion.
- Retained audio: app data dir only when `save_audio = true`.
- Logs: app logs dir, redacted by default.

## MVP Architecture

```text
comlink CLI
  |
  |-- command router
  |-- config manager
  |-- audio capture adapter
  |-- VAD/endpointing
  |-- ASR engine adapter
  |     |-- whisper.cpp subprocess (MVP)
  |     |-- faster-whisper Python worker (optional)
  |     |-- parakeet worker (optional)
  |-- text processing pipeline
  |-- delivery adapter
  |     |-- stdout
  |     |-- clipboard
  |     |-- paste
  |-- SQLite history
```

Recommended MVP implementation:

- Language: Rust or Go for the CLI/daemon if system-wide hotkeys/paste are in scope early; Python/Typer if speed of ASR integration matters more than polished OS integration.
- ASR path: shell out to bundled/system `whisper.cpp` initially. It keeps packaging simple and avoids forcing a Python ML stack on every user.
- Audio capture: PortAudio via `cpal`/Rust, `sounddevice`/Python, or platform-native capture. Convert to 16 kHz mono WAV before ASR.
- File conversion: FFmpeg subprocess.
- Clipboard: use platform-native commands/libraries (`pbcopy`, Windows clipboard API, `wl-copy`/`xclip`) with library fallback.
- Paste: phase in later; copy-to-clipboard is MVP, active-app paste is a daemon feature.

## Audio Pipeline Spec

### Dictation

```text
start trigger
  -> open mic
  -> keep 450 ms ring-buffer pre-roll
  -> stream frames to VAD
  -> write speech frames to temp WAV
  -> stop on user release/Enter or trailing silence
  -> reject if speech < 300 ms
  -> run ASR
  -> deterministic cleanup
  -> optional LLM mode
  -> deliver text
  -> save metadata/history if enabled
```

Requirements:

- Never send empty/silent audio to ASR.
- Pre-roll is RAM-only.
- Show recording state in terminal immediately.
- Always allow manual stop.
- Recordings over `max_record_seconds` stop automatically with a warning.
- Bluetooth mic quirks should be documented; avoid always-open warm mic by default.

### File Transcription

```text
input file
  -> ffprobe metadata
  -> ffmpeg to 16 kHz mono WAV
  -> optional VAD split
  -> ASR per chunk
  -> stitch segments
  -> optional alignment/diarization
  -> export
```

Requirements:

- Preserve source filename and duration in output metadata.
- Support MP3, WAV, M4A, FLAC, OGG, OPUS, MP4, MOV, MKV, WebM.
- Clean temp WAVs even on failure.
- Batch mode should skip completed files unless `--force`.

### Meeting Mode, Early Roadmap

CLI meeting mode should enter the roadmap soon after the useful terminal-first MVP. Phase 2 should provide a lightweight, explicit meeting capture flow; Phase 4 can add pro diarization, richer notes, and searchable transcript library features.

```text
meeting start
  -> visible recording indicator
  -> mic stream, plus system audio when available
  -> VAD and timestamped chunks
  -> rolling transcript preview or segments output
  -> final transcript export
  -> optional source-separated merge
  -> later: pyannote diarization and notes/summary transform
```

Requirements:

- Consent reminder/visible indicator.
- User-controlled start/stop.
- Auto-stop on inactivity.
- Phase 2 may be mic-only on macOS if system audio capture is not ready.
- Source labels before diarization where available: `user_mic`, `system_audio`.
- Do not retain audio by default.
- Save meeting transcripts only when requested or when transcript history retention is enabled.

## Text Processing Spec

### Deterministic Clean Mode

Pipeline:

```text
raw ASR text
  -> trim/collapse whitespace
  -> remove safe fillers: um, uh, umm, uhh
  -> apply custom vocabulary/replacements
  -> extract trailing action snippets
  -> expand snippets
  -> punctuation spacing cleanup
  -> insertion style
```

Rules:

- Do not remove "like", "so", "right", or "you know" by default.
- Replacements use whole-word/phrase matching.
- Longest snippet trigger wins.
- Raw mode still may extract terminal action snippets if user enables voice actions.
- Terminal/CLI mode should preserve exact symbols and avoid sentence punctuation unless requested.

### LLM Rewrite Mode

LLM input should include:

- Raw transcript.
- Clean deterministic transcript.
- Mode instruction.
- Context fields explicitly allowed by config.
- Output contract: final text only, no preamble.

Example prompt template:

```text
You rewrite dictated speech into paste-ready text.
Preserve all technical terms, filenames, commands, URLs, code symbols, and explicit wording.
Do not add facts.
Return only the final text.

Mode: {{mode_instruction}}
Context: {{context_json}}
Transcript: {{clean_text}}
```

Hard requirement:

- Any LLM mode must be skippable with `--no-llm`.
- If the LLM fails, default to deterministic clean text and mark the session with a warning.

## Privacy & Security Spec

Default posture:

- Local-only.
- No network calls during transcription.
- No telemetry.
- No audio retention for dictation.
- Session metadata history on by default.
- Transcript text retention opt-in.
- Active app context allowed by default; clipboard, selected text, textbox content, and screenshots opt-in.

Controls:

```bash
comlink privacy audit
comlink config set privacy.offline true
comlink config set privacy.save_audio false
comlink config set privacy.save_transcripts false
comlink history prune --all
```

Privacy audit should report:

- Installed models and licenses.
- Whether cloud/LLM endpoints are configured.
- Whether session metadata, transcript text, or audio retention is enabled.
- Last network access attempted by the app, if instrumented.
- Configured context permissions.

Implementation guardrails:

- `--offline` blocks model downloads and LLM HTTP requests.
- Do not silently fall back to cloud.
- Do not include transcripts in crash logs.
- Logs should include session IDs and timings, not transcript text, unless debug mode is explicitly enabled.

## Performance Targets

Short dictation:

- Start recording feedback: <100 ms after trigger.
- Pre-roll: 300-500 ms.
- Silence endpoint after speech: 600-1000 ms.
- Empty audio rejection: before ASR.
- Paste/copy after final text: <100 ms.

ASR targets are model/hardware dependent:

- `tiny/base`: near-real-time or faster on most modern CPUs.
- `small.en`: acceptable default quality for developer dictation.
- `turbo/large-v3-turbo`: quality preset, slower on CPU.
- Parakeet: low-latency experimental path for supported languages.
- faster-whisper CUDA: batch/file acceleration path.

UX target:

- The user should feel that recording starts instantly even if transcription completes after release.

## Testing & Evaluation

### Unit Tests

- Config parsing and precedence.
- Vocabulary matching.
- Snippet expansion.
- Filler removal boundaries.
- Terminal action extraction.
- Output format serialization.

### Audio Tests

- Empty WAV rejected.
- Silence-only WAV rejected.
- Very short utterance rejected or warned.
- Pre-roll preserves first syllable.
- VAD splits long audio without dropping speech.
- FFmpeg conversion handles common formats.

### Regression Corpus

Create local test fixtures:

- 20 short dictations with expected text.
- 20 coding prompts with filenames, camelCase, snake_case, CLI commands, URLs.
- 10 noisy/room recordings.
- 10 accents/languages.
- 5 meetings/interviews once diarization exists.

Metrics:

- WER/CER for raw ASR.
- "Paste edit distance" for clean mode.
- Hallucination rate on silence/noise.
- Latency: record stop -> final text.
- Memory by model.
- Offline guarantee: run tests with network disabled.

### Manual UX Tests

- Dictate into terminal, editor, browser, Slack-like text field.
- Trigger hotkey while modifier keys are held.
- Cancel mid-recording.
- Paste after active app changes.
- Clipboard restore behavior.
- Long dictation timeout.

## Build Phases

### Phase 0: Research Spike

- Build `comlink transcribe FILE` around system `whisper.cpp`.
- Add `comlink record` with simple microphone capture.
- Print text to stdout.
- No history, no paste, no daemon.

### Phase 1: Useful Local CLI

- Model install/list/select.
- FFmpeg conversion.
- Silero VAD endpointing.
- Deterministic clean mode.
- Terminal/coding mode preserving shell syntax, filenames, URLs, camelCase, and snake_case.
- Vocabulary and snippets.
- Clipboard copy.
- JSON/Markdown/SRT output.
- SQLite history with transcript retention toggle.

### Phase 2: Dictation Daemon + Meeting Capture v0

- `comlink listen`.
- Global hotkey.
- Recording indicator.
- Copy/paste delivery.
- App-specific mode mapping where OS supports active-app detection.
- `comlink meet start` / `comlink meet stop` with explicit recording state.
- Mic-first meeting capture, with system audio added where OS support is ready.
- Meeting transcript export to Markdown/JSON without diarization.
- Local LLM rewrite through Ollama/llama.cpp.

### Phase 3: Power User + Meeting Polish

- Advanced prompt mode for coding agents.
- Selected text transform mode.
- MCP/local HTTP server for agent integration.
- Per-directory/project vocabulary.
- Git-aware context: repo name, branch, changed files, optional current file.
- Better meeting chunk stitching, source labels, and manual note markers.

### Phase 4: Meeting/File Pro Features

- Diarization via WhisperX/pyannote.
- Mature system audio capture adapters.
- Meeting notes and action extraction.
- Searchable transcript library.
- Export to SRT/VTT/DOCX/Markdown.

## Key Design Decisions

1. Prefer local determinism over cloud polish in the default path.
2. Treat ASR, cleanup, LLM rewrite, and delivery as separate stages.
3. Start with copy-to-clipboard before active paste; paste is OS-specific and failure-prone.
4. Make raw transcript available for every session, even when clean/rewrite mode is used.
5. Never send audio to an LLM. LLMs only receive text.
6. Make context explicit and inspectable.
7. Add diarization later; source-separated capture is often more useful than generic diarization.
8. Use open model/runtime adapters rather than baking one provider into the app.

## Risks

- Real-time Whisper is chunked and can produce overlap/stitching artifacts.
- Local LLM rewrites can alter meaning; deterministic mode must remain the fallback.
- Cross-platform paste is deceptively hard, especially Wayland and elevated/admin windows.
- Model licensing differs; do not redistribute models without checking terms.
- Speaker diarization can require gated model access and is not always commercially simple.
- Keeping a microphone warm improves UX but may show OS mic indicators and affect Bluetooth audio quality.
- Empty/noisy audio can trigger hallucinations in Whisper-like models unless VAD and minimum-duration guards are strong.

## Recommended Initial Stack

For fastest path to a working CLI:

- CLI: Rust + Clap.
- ASR: `whisper.cpp` subprocess.
- Audio capture: `cpal` for Rust.
- Conversion: FFmpeg.
- VAD: Silero through ONNX Runtime or a worker adapter initially; consider a tighter Rust integration later.
- Storage: SQLite.
- Clipboard: platform adapters.
- Local LLM: Ollama first, OpenAI-compatible endpoint second.

For a polished cross-platform daemon:

- Core CLI/daemon in Rust.
- Native helper modules for paste/hotkey where libraries are insufficient.
- ASR engines as subprocess/worker adapters.
- Python worker optional for faster-whisper/WhisperX.

## Source Index

Product docs and apps:

- [Wispr Flow docs: What is Flow?](https://docs.wisprflow.ai/articles/2772472373-what-is-flow)
- [Wispr Flow features](https://wisprflow.ai/features)
- [Wispr Flow data controls](https://wisprflow.ai/data-controls)
- [Superwhisper docs](https://superwhisper.com/docs/get-started/introduction)
- [Superwhisper modes](https://superwhisper.com/docs/modes/modes)
- [Superwhisper voice models](https://superwhisper.com/docs/models/voice)
- [Aqua Voice](https://aquavoice.com/)
- [Granola transcription docs](https://docs.granola.ai/help-center/taking-notes/transcription)
- [Granola security](https://www.granola.ai/security)
- [MacWhisper](https://www.macwhisper.com/)
- [Aiko](https://sindresorhus.com/aiko)
- [VoiceInk](https://github.com/beingpax/VoiceInk)
- [OpenWhispr](https://github.com/OpenWhispr/openwhispr)
- [MacParakeet](https://github.com/moona3k/macparakeet)
- [Spokenly](https://spokenly.app/)

Models/runtimes:

- [OpenAI Whisper](https://github.com/openai/whisper)
- [Whisper model card](https://github.com/openai/whisper/blob/main/model-card.md)
- [whisper.cpp](https://github.com/ggml-org/whisper.cpp)
- [faster-whisper](https://github.com/SYSTRAN/faster-whisper)
- [WhisperX](https://github.com/m-bain/whisperX)
- [NVIDIA Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)
- [NVIDIA Parakeet Unified EN 0.6B](https://huggingface.co/nvidia/parakeet-unified-en-0.6b)
- [NVIDIA Canary 1B v2](https://huggingface.co/nvidia/canary-1b-v2)
- [Silero VAD](https://github.com/snakers4/silero-vad)
- [pyannote.audio](https://github.com/pyannote/pyannote-audio)
- [Ollama API](https://docs.ollama.com/api/introduction)
- [llama.cpp server](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
