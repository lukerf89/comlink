# Local Install and Dev Run

Comlink is a local-first CLI. The default transcription path uses local FFmpeg,
local whisper.cpp, and a local ggml model file.

## Install for Local Use

Build and install the current checkout:

```bash
cargo install --path .
```

Or run without installing:

```bash
cargo run -- doctor --format json
cargo run -- transcribe tests/fixtures/audio/short.wav --mode memo --format json
```

## Required Runtime Tools

Set explicit paths when binaries are not on `PATH`:

```bash
export COMLINK_FFMPEG="/opt/homebrew/bin/ffmpeg"
export COMLINK_FFPROBE="/opt/homebrew/bin/ffprobe"
export COMLINK_WHISPER_CPP="/path/to/whisper-cli"
export COMLINK_WHISPER_MODEL="/path/to/ggml-model.bin"
```

You can also persist a model selection:

```bash
comlink models select tiny --path /path/to/ggml-tiny.en.bin
```

For meeting capture, prefer at least `ggml-base.en.bin` or `ggml-small.en.bin`
when your machine can handle it. `tiny.en` is useful for fast smoke tests, but
it is much more likely to hallucinate or repeat phrases in noisy rooms.

Run diagnostics after changing paths:

```bash
comlink doctor
comlink doctor --format json
```

## Recording and Clipboard

Record from the default macOS AVFoundation microphone device:

```bash
comlink record --mode memo --copy --format json
```

If the default device is wrong, override it:

```bash
COMLINK_RECORD_DEVICE=":1" comlink record --mode memo --copy
```

Clipboard delivery uses `pbcopy` by default. Tests and custom environments can
override it:

```bash
COMLINK_PBCOPY=/path/to/pbcopy-compatible-command comlink record --copy
```

`--restore-clipboard` is opt-in and requires `--copy`. It reads the previous
clipboard through `pbpaste` or `COMLINK_PBPASTE`, writes the final text, then
restores the previous clipboard. This is useful for adapter testing and future
paste workflows; the default copy-first workflow leaves the final text on the
clipboard.

## Work Surface Modes

Built-in deterministic modes include:

- `terminal`: single-line command or agent prompt text; dictated line breaks
  become `&&` separators and trailing sentence punctuation is removed.
- `editor`: editor-safe prose that honors dictated `new line`.
- `outlook`: email-composer prose that honors dictated `new paragraph`.
- `slack`: chat-composer text that stays concise.
- `memo`: note text with sentence-ending punctuation.

Examples:

```bash
comlink modes apply --mode terminal --text "cargo test --all new line git status --short"
comlink modes apply --mode outlook --text "thanks new paragraph I can review today"
```

## Agent Integration: `comlink mcp` (Phase 10b)

`comlink mcp` is a local stdio [MCP](https://modelcontextprotocol.io) server.
The MCP client (Claude Code, Claude Desktop) starts it as a subprocess and
talks JSON-RPC over its stdin/stdout. It opens no network socket, and there is
no remote or HTTP transport. Sessions live in the same meeting store as the
CLI, so a meeting started by an agent can be stopped with `comlink meet stop`
and the reverse. A recording keeps going if the client or server restarts.

Tools: `meeting_start`, `meeting_status`, `meeting_stop` (always detached:
poll `meeting_status` until `stopped`), `meeting_get_transcript` and
`meeting_list`. Resources: `comlink://meetings/{id}/transcript.md` and
`comlink://meetings/{id}/transcript.json`. See `docs/output-contract.md` for
the schemas and error codes.

### 1. Install and allow start

Install a stable binary (an MCP client should not point at a `target/` dir
that `cargo clean` removes), then opt in to agent-started recordings:

```bash
cargo install --path .
comlink config set mcp.allow_start true   # default false: meeting_start is refused
comlink doctor                            # shows mcp-server (binary path) and mcp-allow-start
```

`COMLINK_MCP_ALLOW_START=true|false` overrides the config file for one
process; `config set` warns when the variable is set and disagrees (or is not
a boolean, which makes every config load fail). Status, stop, list and
transcript reads work regardless of `mcp.allow_start`.

`config set` rewrites `config.json` atomically (temp file + rename, mode
0600), so a running `comlink mcp` never reads a half-written file. If
`config.json` is a symlink (for example into a dotfiles repo), the link is
replaced by a regular file; edit the link target by hand instead.

MCP clients do not inherit your shell profile. Persist the model with
`comlink models select base --path /path/to/ggml-base.en.bin`, and pass tool
paths that are not on the default `PATH` (Homebrew's `/opt/homebrew/bin` often
is not) as environment variables in the registration below.

### 2. Register with Claude Code

```bash
claude mcp add comlink \
  -e COMLINK_FFMPEG=/opt/homebrew/bin/ffmpeg \
  -e COMLINK_WHISPER_CPP=/opt/homebrew/bin/whisper-cli \
  -- "$(command -v comlink)" mcp
claude mcp list
```

### 3. Register with Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`, then
restart Claude Desktop:

```json
{
  "mcpServers": {
    "comlink": {
      "command": "/Users/you/.cargo/bin/comlink",
      "args": ["mcp"],
      "env": {
        "COMLINK_FFMPEG": "/opt/homebrew/bin/ffmpeg",
        "COMLINK_WHISPER_CPP": "/opt/homebrew/bin/whisper-cli"
      }
    }
  }
}
```

### Microphone permission (TCC)

macOS grants microphone access to the app that launches `comlink mcp`, not to
`comlink` itself: your terminal app for Claude Code, and Claude.app for Claude
Desktop. The first recording may trigger the permission prompt for that app;
grant it in System Settings, Privacy & Security, Microphone. Without it,
recordings are near-silent and the agent sees a `near-silent` warning in
`meeting_status` and `meeting_get_transcript`. Check the input from the same
app with `comlink doctor --probe-mic`.

### Privacy

- `meeting_get_transcript` and the transcript resources send transcript text
  to the calling model. Do not register the server with a client you would
  not show your meetings to.
- The server logs nothing and never writes transcript text to stderr; stdout
  carries only JSON-RPC frames.
- `comlink privacy audit` reports the MCP transport (`stdio`), that there is
  no network listener, and the current `allow_start` value.

## Deferred From Phase 6

Global hotkey, active paste, and app-specific automation remain deferred. Phase 6
keeps the supported delivery model copy-first so failures are visible, local,
and easy to recover from.
