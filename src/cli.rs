use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::{
    asr::{AsrEngine, SourceMetadata, WhisperCppEngine},
    audio, clipboard,
    config::{self, CliConfigOverrides, ConfigFormat},
    deps, doctor,
    error::ComlinkError,
    meet,
    meet_service::{
        self, MeetContext, MeetDetachedStatus, MeetStartStatus, MeetStatusReport, MeetStopStatus,
    },
    output::{self, ContextMetadata, OutputFormat},
    record,
    storage::{self, PruneResult, StoredSegment, StoredSession, StoredSessionSummary},
    system_audio, text,
};

const DEFAULT_MIN_RECORDING_MS: u64 = 300;
const DEFAULT_MEETING_CHUNK_SECONDS: u64 = meet_service::DEFAULT_CHUNK_SECONDS;
const DEFAULT_MEETING_STOP_TIMEOUT_SECONDS: u64 = 15;
const DEFAULT_FINALIZE_LOCK_WAIT_SECONDS: u64 = 30;

#[derive(Debug, Parser)]
#[command(name = "comlink")]
#[command(about = "Local-first speech and audio transcription CLI")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check local runtime dependencies and model configuration.
    Doctor {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,

        /// Run a short (1.5s) live microphone capture and report whether the
        /// record input device has signal. Opt-in: touches the microphone.
        #[arg(long)]
        probe_mic: bool,
    },

    /// Inspect resolved local configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },

    /// Inspect and prune saved transcript history.
    History {
        #[command(subcommand)]
        command: HistoryCommand,
    },

    /// Inspect and select local model registry entries.
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },

    /// Inspect and apply deterministic text work modes.
    Modes {
        #[command(subcommand)]
        command: ModesCommand,
    },

    /// Manage local style profiles for LLM rewrite modes.
    Styles {
        #[command(subcommand)]
        command: StylesCommand,
    },

    /// Manage local vocabulary replacements.
    Vocab {
        #[command(subcommand)]
        command: VocabCommand,
    },

    /// Manage local text snippets.
    Snippets {
        #[command(subcommand)]
        command: SnippetsCommand,
    },

    /// Inspect local privacy posture.
    Privacy {
        #[command(subcommand)]
        command: PrivacyCommand,
    },

    /// Transcribe an audio or video file with local whisper.cpp.
    Transcribe {
        /// Audio or video file to transcribe.
        file: PathBuf,

        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,

        /// Text processing mode.
        #[arg(long, default_value = "raw")]
        mode: String,

        /// whisper.cpp ggml model path. Defaults to COMLINK_WHISPER_MODEL.
        #[arg(long)]
        model: Option<PathBuf>,

        /// Save this transcript to local history when history is enabled.
        #[arg(long)]
        save: bool,

        /// Skip any configured local LLM rewrite and use deterministic output.
        #[arg(long)]
        no_llm: bool,
    },

    /// Record a short microphone memo and transcribe it locally.
    Record {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,

        /// Text processing mode.
        #[arg(long, default_value = "memo")]
        mode: String,

        /// Copy final text to the macOS clipboard.
        #[arg(long)]
        copy: bool,

        /// Restore the previous clipboard after a successful copy. Requires --copy.
        #[arg(long, requires = "copy")]
        restore_clipboard: bool,

        /// whisper.cpp ggml model path. Defaults to COMLINK_WHISPER_MODEL.
        #[arg(long)]
        model: Option<PathBuf>,

        /// Minimum captured duration before ASR runs.
        #[arg(long, default_value_t = DEFAULT_MIN_RECORDING_MS)]
        min_duration_ms: u64,

        /// FFmpeg AVFoundation input device, by name (e.g. "MacBook Pro Microphone") or index (:2). Defaults to COMLINK_RECORD_DEVICE, else the system default input device (fallback :0).
        #[arg(long)]
        device: Option<String>,

        /// Save this recording transcript to local history when history is enabled.
        #[arg(long)]
        save: bool,

        /// Skip any configured local LLM rewrite and use deterministic output.
        #[arg(long)]
        no_llm: bool,
    },

    /// Capture and export local meeting transcripts.
    Meet {
        #[command(subcommand)]
        command: MeetCommand,
    },

    /// Run the local stdio MCP server for agents (Claude Code, Claude
    /// Desktop). The MCP client launches this; stdout carries only JSON-RPC.
    Mcp,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Show resolved config, paths, and source precedence.
    Show {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },

    /// Set a persistent config value. Supported key: mcp.allow_start.
    Set {
        /// Config key (only `mcp.allow_start`).
        key: String,

        /// Value (`true`/`false`).
        value: String,
    },
}

#[derive(Debug, Subcommand)]
enum HistoryCommand {
    /// List saved history sessions.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },

    /// Show one saved history session.
    Show {
        /// Saved session id.
        id: String,

        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Remove saved history records.
    Prune {
        /// Remove all saved sessions and segments.
        #[arg(long)]
        all: bool,

        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },
}

#[derive(Debug, Subcommand)]
enum ModelsCommand {
    /// List configured local model registry entries.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },

    /// Register and select a local model path.
    Select {
        /// Registry name for this local model.
        name: String,

        /// ggml model path for this registry entry.
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum ModesCommand {
    /// List built-in and configured modes.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },

    /// Add or update a configured local mode.
    Add {
        /// Mode name, such as prompt.
        name: String,

        /// Mode-level local LLM instruction.
        #[arg(long)]
        instruction: String,

        /// Deterministic fallback mode used before any LLM rewrite.
        #[arg(long, default_value = "memo")]
        deterministic_mode: String,

        /// Human-readable description.
        #[arg(long)]
        description: Option<String>,

        /// Optional style profile name to include in local LLM requests.
        #[arg(long)]
        style_profile: Option<String>,
    },

    /// Process plain text through a mode without ASR.
    Apply {
        /// Text processing mode.
        #[arg(long)]
        mode: String,

        /// Text to process.
        #[arg(long)]
        text: String,

        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },

    /// Remove a configured local mode.
    Remove {
        /// Mode name to remove.
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum StylesCommand {
    /// Import a style profile JSON file.
    Import {
        /// Structured profile file with name, summary, and examples.
        file: PathBuf,

        /// Override the profile name from the file.
        #[arg(long)]
        name: Option<String>,
    },

    /// List configured style profiles.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },
}

#[derive(Debug, Subcommand)]
enum VocabCommand {
    /// Add or update a vocabulary replacement.
    Add {
        /// Dictated phrase to replace.
        phrase: String,

        /// Final written replacement.
        replacement: String,
    },

    /// List configured vocabulary replacements.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },

    /// Remove a vocabulary replacement by phrase.
    Remove {
        /// Dictated phrase to remove.
        phrase: String,
    },
}

#[derive(Debug, Subcommand)]
enum SnippetsCommand {
    /// Add or update a snippet trigger.
    Add {
        /// Dictated phrase that expands the snippet.
        trigger: String,

        /// Snippet body. Literal \n sequences are saved as newlines.
        body: String,
    },

    /// List configured snippets.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },

    /// Remove a snippet by trigger.
    Remove {
        /// Dictated trigger to remove.
        trigger: String,
    },
}

#[derive(Debug, Subcommand)]
enum PrivacyCommand {
    /// Show local retention, model, and LLM posture.
    Audit {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },
}

#[derive(Debug, Subcommand)]
enum MeetCommand {
    /// Start a chunked meeting recording.
    Start {
        /// Status output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,

        /// Text processing mode applied to the final transcript after stop.
        #[arg(long, default_value = "raw")]
        mode: String,

        /// whisper.cpp ggml model path. Defaults to COMLINK_WHISPER_MODEL.
        #[arg(long)]
        model: Option<PathBuf>,

        /// FFmpeg AVFoundation input device, by name (e.g. "MacBook Pro Microphone") or index (:2). Defaults to COMLINK_RECORD_DEVICE, else the system default input device (fallback :0).
        #[arg(long)]
        device: Option<String>,

        /// Meeting capture source. Defaults to Phase 7-compatible mic-only.
        #[arg(long, value_parser = ["mic-only", "system-only", "mic-plus-system"], default_value = "mic-only")]
        source: String,

        /// BlackHole FFmpeg AVFoundation audio input, by name (e.g. "BlackHole 2ch") or index (:2). Defaults to COMLINK_SYSTEM_AUDIO_DEVICE or detected BlackHole.
        #[arg(long)]
        system_device: Option<String>,

        /// Chunk size for long-form capture.
        #[arg(long, default_value_t = DEFAULT_MEETING_CHUNK_SECONDS)]
        chunk_seconds: u64,

        /// Skip any configured local LLM rewrite and use deterministic output.
        #[arg(long)]
        no_llm: bool,
    },

    /// Stop a meeting recording and write transcript artifacts.
    Stop {
        /// Meeting session id. Defaults to the active session.
        id: Option<String>,

        /// Status output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,

        /// Seconds to wait for the recorder to flush its final chunk.
        #[arg(long, default_value_t = DEFAULT_MEETING_STOP_TIMEOUT_SECONDS)]
        wait_timeout_seconds: u64,

        /// Stop the recorders, mark the session `transcribing`, and return
        /// immediately; a detached `meet finalize` writes the transcript.
        #[arg(long)]
        detach: bool,
    },

    /// Report the active (or given) meeting session: recorder health, chunk
    /// count, latest audio level, and stale detection. Exits 0 when there is no
    /// session (status `none`).
    Status {
        /// Meeting session id. Defaults to the active recording session, then
        /// the newest transcribing session.
        id: Option<String>,

        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },

    /// Internal: finish transcription for a session stopped with `--detach`.
    /// Safe to rerun; on a stopped session it reprints the stop status without
    /// re-transcribing.
    Finalize {
        /// Meeting session id.
        id: String,

        /// Status output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,

        /// Seconds to wait for another comlink process to release the session.
        #[arg(long, default_value_t = DEFAULT_FINALIZE_LOCK_WAIT_SECONDS)]
        lock_wait_seconds: u64,
    },

    /// Print a stopped meeting transcript export.
    Export {
        /// Meeting session id. Defaults to the most recent stopped session.
        id: Option<String>,

        /// Export format.
        #[arg(long, value_enum, default_value = "md")]
        format: MeetExportFormat,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MeetExportFormat {
    Json,
    Md,
}

pub fn run() -> Result<(), ComlinkError> {
    let cli = Cli::parse();

    match cli.command {
        Command::Doctor { format, probe_mic } => {
            let healthy = doctor::run(format, doctor::DoctorOptions { probe_mic })?;
            if healthy {
                Ok(())
            } else {
                Err(ComlinkError::DependencyMissing(
                    "one or more required dependencies",
                ))
            }
        }
        Command::Config { command } => run_config(command),
        Command::History { command } => run_history(command),
        Command::Models { command } => run_models(command),
        Command::Modes { command } => run_modes(command),
        Command::Styles { command } => run_styles(command),
        Command::Vocab { command } => run_vocab(command),
        Command::Snippets { command } => run_snippets(command),
        Command::Privacy { command } => run_privacy(command),
        Command::Transcribe {
            file,
            format,
            mode,
            model,
            save,
            no_llm,
        } => transcribe(file, format, &mode, model, save, no_llm),
        Command::Record {
            format,
            mode,
            copy,
            restore_clipboard,
            model,
            min_duration_ms,
            device,
            save,
            no_llm,
        } => record_memo(RecordMemoOptions {
            format,
            mode: &mode,
            copy,
            restore_clipboard,
            model,
            min_duration_ms,
            device,
            save,
            no_llm,
        }),
        Command::Meet { command } => run_meet(command),
        Command::Mcp => crate::mcp::serve_stdio(),
    }
}

fn transcribe(
    file: PathBuf,
    format: OutputFormat,
    mode: &str,
    model: Option<PathBuf>,
    save: bool,
    no_llm: bool,
) -> Result<(), ComlinkError> {
    let resolved = config::load(CliConfigOverrides { model })?;
    validate_requested_mode(&resolved.config, mode)?;
    let model_path =
        config::selected_model_path(&resolved.config).ok_or(ComlinkError::ModelMissing)?;
    let runtime = deps::runtime_from_model_path(model_path)?;
    let normalized = audio::normalize_to_wav(&file, &runtime.ffmpeg, runtime.ffprobe.as_deref())?;
    let engine = WhisperCppEngine {
        binary: runtime.whisper_cpp,
        model: runtime.whisper_model,
    };
    let source = SourceMetadata {
        path: file.display().to_string(),
        normalized_sample_rate_hz: normalized.sample_rate_hz,
        normalized_channels: normalized.channels,
    };

    let transcript = engine.transcribe(&normalized.path, source, normalized.duration_ms)?;
    let mut transcript = output::TranscriptOutput::from_transcript(
        transcript,
        mode,
        false,
        &resolved.config,
        no_llm,
    )?;
    maybe_save_transcript(&resolved, &mut transcript, save, Some(&normalized.path))?;
    output::print_transcript(&transcript, format)
}

struct RecordMemoOptions<'a> {
    format: OutputFormat,
    mode: &'a str,
    copy: bool,
    restore_clipboard: bool,
    model: Option<PathBuf>,
    min_duration_ms: u64,
    device: Option<String>,
    save: bool,
    no_llm: bool,
}

fn record_memo(options: RecordMemoOptions<'_>) -> Result<(), ComlinkError> {
    let RecordMemoOptions {
        format,
        mode,
        copy,
        restore_clipboard,
        model,
        min_duration_ms,
        device,
        save,
        no_llm,
    } = options;
    let resolved = config::load(CliConfigOverrides { model })?;
    validate_requested_mode(&resolved.config, mode)?;
    let model_path =
        config::selected_model_path(&resolved.config).ok_or(ComlinkError::ModelMissing)?;
    let runtime = deps::runtime_from_model_path(model_path)?;
    let resolved_device = resolve_record_device_full(device, &runtime.ffmpeg)?;
    let device = resolved_device.avfoundation_input.clone();

    eprintln!("Recording... press Enter to stop.");
    let captured = record::record_until_enter(record::RecordingOptions {
        ffmpeg: &runtime.ffmpeg,
        ffprobe: runtime.ffprobe.as_deref(),
        device: &device,
    })?;

    if captured.duration_ms < min_duration_ms {
        return Err(ComlinkError::RecordingTooShort {
            duration_ms: captured.duration_ms,
            min_duration_ms,
        });
    }

    // Best-effort level check: an unmeasurable WAV (None) keeps legacy behavior.
    let level = audio::read_wav_level_samples(&captured.path)
        .and_then(|samples| audio::session_audio_level([samples]));
    let near_silent = record::diagnose_record_level(level);
    let near_silent_warning = near_silent.map(|level| {
        let warning = audio::near_silent_warning_message(level.mean_dbfs);
        eprintln!("warning: {warning}");
        eprintln!(
            "{}",
            record::near_silent_hint(&resolved_device, &runtime.ffmpeg)
        );
        warning
    });

    let engine = WhisperCppEngine {
        binary: runtime.whisper_cpp,
        model: runtime.whisper_model,
    };
    let source = SourceMetadata {
        path: "microphone".to_string(),
        normalized_sample_rate_hz: captured.sample_rate_hz,
        normalized_channels: captured.channels,
    };

    let transcript = engine
        .transcribe(&captured.path, source, captured.duration_ms)
        .map_err(|error| {
            record::map_empty_transcript(error, near_silent, &resolved_device.label())
        })?;
    let mut transcript = output::TranscriptOutput::from_transcript(
        transcript,
        mode,
        copy,
        &resolved.config,
        no_llm,
    )?;
    if let Some(warning) = near_silent_warning {
        transcript.warnings.push(warning);
    }
    let stop_to_final_ms = captured.stopped_at.elapsed().as_millis();

    if copy {
        let copy_result = clipboard::copy_text_with_options(
            &transcript.final_text,
            clipboard::CopyOptions {
                restore_previous: restore_clipboard,
            },
        )?;
        if copy_result.restored_previous {
            eprintln!("Copied final text to clipboard, then restored previous clipboard.");
        } else {
            eprintln!("Copied final text to clipboard.");
        }
    }

    maybe_save_transcript(&resolved, &mut transcript, save, Some(&captured.path))?;
    eprintln!("Stop-to-final latency: {} ms.", stop_to_final_ms);
    output::print_transcript(&transcript, format)
}

fn validate_requested_mode(config: &config::Config, mode: &str) -> Result<(), ComlinkError> {
    text::validate_mode(config, mode)
}

fn run_config(command: ConfigCommand) -> Result<(), ComlinkError> {
    match command {
        ConfigCommand::Show { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            config::print(&resolved, format)
        }
        ConfigCommand::Set { key, value } => {
            let outcome = config::set_key(&key, &value)?;
            println!(
                "set {}={} in {}",
                outcome.key,
                outcome.value,
                outcome.config_file.display()
            );
            match outcome.env_override {
                Some(env_value) if outcome.env_override_valid => eprintln!(
                    "note: {}={env_value} is set in the environment and takes precedence over the config file",
                    config::MCP_ALLOW_START_ENV
                ),
                Some(env_value) => eprintln!(
                    "warning: {}={env_value} is set in the environment but is not a boolean; every comlink command (and the MCP server) will fail to load config until it is fixed or unset",
                    config::MCP_ALLOW_START_ENV
                ),
                None => {}
            }
            Ok(())
        }
    }
}

fn run_history(command: HistoryCommand) -> Result<(), ComlinkError> {
    let resolved = config::load(CliConfigOverrides::default())?;
    match command {
        HistoryCommand::List { format } => {
            let sessions = storage::list(&resolved.paths)?;
            print_history_list(&sessions, format)
        }
        HistoryCommand::Show { id, format } => {
            let session = storage::show(&resolved.paths, &id)?;
            print_history_session(&session, format)
        }
        HistoryCommand::Prune { all, format } => {
            if !all {
                return Err(ComlinkError::InvalidConfigValue {
                    name: "history prune",
                    value: "pass --all to prune all history".to_string(),
                });
            }
            let result = storage::prune_all(&resolved.paths)?;
            print_prune_result(&result, format)
        }
    }
}

fn run_models(command: ModelsCommand) -> Result<(), ComlinkError> {
    match command {
        ModelsCommand::List { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            print_models(&resolved, format)
        }
        ModelsCommand::Select { name, path } => {
            if !path.is_file() {
                return Err(ComlinkError::ModelPathMissing(path));
            }
            let path = path.canonicalize()?;
            let selected = config::update_persistent(|resolved| {
                config::select_model(resolved, &name, path);
                Ok(resolved.config.selected_model.clone())
            })?;
            println!(
                "selected model: {}",
                selected.as_deref().unwrap_or("<none>")
            );
            Ok(())
        }
    }
}

fn run_modes(command: ModesCommand) -> Result<(), ComlinkError> {
    match command {
        ModesCommand::List { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            print_modes(&resolved.config, format)
        }
        ModesCommand::Add {
            name,
            instruction,
            deterministic_mode,
            description,
            style_profile,
        } => {
            if text::TextMode::parse(&deterministic_mode).is_none() {
                return Err(ComlinkError::ModeNotFound(deterministic_mode));
            }
            config::update_persistent(|resolved| {
                config::upsert_mode(
                    &mut resolved.config,
                    name.clone(),
                    description,
                    Some(deterministic_mode),
                    Some(instruction),
                    style_profile,
                );
                Ok(())
            })?;
            println!("saved mode: {name}");
            Ok(())
        }
        ModesCommand::Apply { mode, text, format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            let processed = output::process_text(&text, &mode, &resolved.config, true)?;
            let output = ProcessedTextOutput {
                raw_text: text,
                final_text: processed.final_text,
                mode: processed.mode,
                processing_steps: processed
                    .processing_steps
                    .into_iter()
                    .map(|step| step.name)
                    .collect(),
            };
            print_processed_text(&output, format)
        }
        ModesCommand::Remove { name } => {
            config::update_persistent(|resolved| {
                if !config::remove_mode(&mut resolved.config, &name) {
                    return Err(ComlinkError::NotFound { kind: "mode", name });
                }
                Ok(())
            })?;
            println!("removed mode");
            Ok(())
        }
    }
}

fn run_styles(command: StylesCommand) -> Result<(), ComlinkError> {
    match command {
        StylesCommand::Import { file, name } => {
            let mut profile: config::StyleProfile =
                serde_json::from_str(&fs::read_to_string(&file).map_err(ComlinkError::from)?)?;
            if let Some(name) = name {
                profile.name = name;
            }
            let profile_name = profile.name.clone();
            config::update_persistent(|resolved| {
                config::upsert_style_profile(&mut resolved.config, profile);
                Ok(())
            })?;
            println!("saved style profile: {profile_name}");
            Ok(())
        }
        StylesCommand::List { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            print_style_profiles(&resolved.config.style_profiles, format)
        }
    }
}

fn run_vocab(command: VocabCommand) -> Result<(), ComlinkError> {
    match command {
        VocabCommand::Add {
            phrase,
            replacement,
        } => {
            config::update_persistent(|resolved| {
                config::upsert_vocabulary(
                    &mut resolved.config,
                    phrase.clone(),
                    replacement.clone(),
                );
                Ok(())
            })?;
            println!("{phrase} -> {replacement}");
            Ok(())
        }
        VocabCommand::List { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            print_vocabulary(&resolved.config.vocabulary, format)
        }
        VocabCommand::Remove { phrase } => {
            config::update_persistent(|resolved| {
                if !config::remove_vocabulary(&mut resolved.config, &phrase) {
                    return Err(ComlinkError::NotFound {
                        kind: "vocabulary",
                        name: phrase,
                    });
                }
                Ok(())
            })?;
            println!("removed vocabulary phrase");
            Ok(())
        }
    }
}

fn run_snippets(command: SnippetsCommand) -> Result<(), ComlinkError> {
    match command {
        SnippetsCommand::Add { trigger, body } => {
            let body = decode_cli_newlines(&body);
            config::update_persistent(|resolved| {
                config::upsert_snippet(&mut resolved.config, trigger.clone(), body);
                Ok(())
            })?;
            println!("saved snippet: {trigger}");
            Ok(())
        }
        SnippetsCommand::List { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            print_snippets(&resolved.config.snippets, format)
        }
        SnippetsCommand::Remove { trigger } => {
            config::update_persistent(|resolved| {
                if !config::remove_snippet(&mut resolved.config, &trigger) {
                    return Err(ComlinkError::NotFound {
                        kind: "snippet",
                        name: trigger,
                    });
                }
                Ok(())
            })?;
            println!("removed snippet");
            Ok(())
        }
    }
}

fn run_privacy(command: PrivacyCommand) -> Result<(), ComlinkError> {
    match command {
        PrivacyCommand::Audit { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            let selected_model = config::selected_model_path(&resolved.config);
            let dependencies = deps::inspect_with_model_path(selected_model.clone());
            let system_audio_report = system_audio::inspect(&dependencies);
            let meeting_audio =
                meet_service::meeting_audio_audit(&MeetContext::new(resolved.clone(), None));
            let mcp = meet_service::mcp_privacy(&resolved);
            let audit = PrivacyAudit {
                history_enabled: resolved.config.history_enabled,
                retention: resolved.config.retention.clone(),
                config_file: resolved.paths.config_file.display().to_string(),
                database_file: resolved.paths.database_file.display().to_string(),
                selected_model: selected_model
                    .as_ref()
                    .map(|path| path.display().to_string()),
                selected_model_exists: selected_model
                    .as_ref()
                    .map(|path| path.is_file())
                    .unwrap_or(false),
                asr: "local whisper.cpp".to_string(),
                llm: llm_privacy_status(&resolved.config.llm),
                system_audio: PrivacySystemAudio {
                    strategy: system_audio_report.strategy,
                    available: system_audio_report.available,
                    status: system_audio_report.status,
                    device_name: resolved
                        .config
                        .retention
                        .metadata
                        .then_some(system_audio_report.dependency.device_name)
                        .flatten(),
                    microphone_permission: system_audio_report.permissions.microphone.detail,
                    routing_permission: system_audio_report.permissions.system_audio_routing.detail,
                    raw_audio_retained: resolved.config.retention.audio,
                },
                meeting_audio,
                mcp,
            };
            print_privacy_audit(&audit, format)
        }
    }
}

fn run_meet(command: MeetCommand) -> Result<(), ComlinkError> {
    match command {
        MeetCommand::Start {
            format,
            mode,
            model,
            device,
            source,
            system_device,
            chunk_seconds,
            no_llm,
        } => meet_start(MeetStartOptions {
            format,
            mode: &mode,
            model,
            device,
            source: &source,
            system_device,
            chunk_seconds,
            no_llm,
        }),
        MeetCommand::Stop {
            id,
            format,
            wait_timeout_seconds,
            detach,
        } => meet_stop(MeetStopOptions {
            id,
            format,
            wait_timeout_seconds,
            detach,
        }),
        MeetCommand::Status { id, format } => meet_status(id, format),
        MeetCommand::Finalize {
            id,
            format,
            lock_wait_seconds,
        } => meet_finalize(&id, format, lock_wait_seconds),
        MeetCommand::Export { id, format } => meet_export(id, format),
    }
}

struct MeetStartOptions<'a> {
    format: ConfigFormat,
    mode: &'a str,
    model: Option<PathBuf>,
    device: Option<String>,
    source: &'a str,
    system_device: Option<String>,
    chunk_seconds: u64,
    no_llm: bool,
}

fn meet_start(options: MeetStartOptions<'_>) -> Result<(), ComlinkError> {
    let MeetStartOptions {
        format,
        mode,
        model,
        device,
        source,
        system_device,
        chunk_seconds,
        no_llm,
    } = options;
    let resolved = config::load(CliConfigOverrides { model })?;
    let prepared = meet_service::prepare_start(
        &resolved,
        None,
        meet_service::StartOptions {
            mode: mode.to_string(),
            source: source.to_string(),
            device,
            system_device,
            chunk_seconds,
            no_llm,
        },
    )?;
    let (ctx, request, device_note) = prepared.into_context(resolved);
    if let Some(note) = device_note {
        eprintln!("{note}");
    }
    let status = meet_service::start(&ctx, request)?;

    eprintln!("{}", status.consent_reminder);
    print_meet_start(&status, format)
}

/// Resolve the microphone capture device for `record` / `meet start` via the
/// shared [`record::resolve_record_device`] (same precedence `doctor` reports),
/// announcing a resolved system default on stderr.
fn resolve_record_device_full(
    device: Option<String>,
    ffmpeg: &Path,
) -> Result<record::ResolvedRecordDevice, ComlinkError> {
    let resolved = record::resolve_record_device(device, ffmpeg)?;
    if resolved.source == record::DeviceSource::SystemDefault {
        match &resolved.name {
            Some(name) => eprintln!(
                "Using system default input device: {name} ({})",
                resolved.avfoundation_input
            ),
            None => eprintln!(
                "Using system default input device {}",
                resolved.avfoundation_input
            ),
        }
    }
    Ok(resolved)
}

struct MeetStopOptions {
    id: Option<String>,
    format: ConfigFormat,
    wait_timeout_seconds: u64,
    detach: bool,
}

fn meet_stop(options: MeetStopOptions) -> Result<(), ComlinkError> {
    let MeetStopOptions {
        id,
        format,
        wait_timeout_seconds,
        detach,
    } = options;
    let resolved = config::load(CliConfigOverrides::default())?;
    let ctx = MeetContext::new(resolved, None);
    let session = meet_service::prepare_stop(&ctx, id)?;

    eprintln!("Stopping meeting recording: {}", session.session_id);
    let wait_timeout = Duration::from_secs(wait_timeout_seconds);
    if detach {
        let status = meet_service::stop_detached_prepared(&ctx, &session.session_id, wait_timeout)?;
        eprintln!(
            "Transcribing in the background (finalizer pid {}); check progress with `comlink meet status {}`.",
            status.finalizer_pid, status.session_id
        );
        return print_meet_detached(&status, format);
    }

    let status = meet_service::stop_prepared(&ctx, session, wait_timeout)?;
    emit_stop_warning_banner(&status.warnings);
    print_meet_stop(&status, format)
}

fn meet_status(id: Option<String>, format: ConfigFormat) -> Result<(), ComlinkError> {
    let resolved = config::load(CliConfigOverrides::default())?;
    let ctx = MeetContext::new(resolved, None);
    let report = meet_service::status(&ctx, id)?;
    print_meet_status(&report, format)
}

fn meet_finalize(
    id: &str,
    format: ConfigFormat,
    lock_wait_seconds: u64,
) -> Result<(), ComlinkError> {
    let resolved = config::load(CliConfigOverrides::default())?;
    let ctx = MeetContext::new(resolved, None);
    let status = meet_service::finalize(&ctx, id, Duration::from_secs(lock_wait_seconds))?;
    emit_stop_warning_banner(&status.warnings);
    print_meet_stop(&status, format)
}

/// Print any meeting warnings as a clearly delimited banner on stderr so a
/// near-silent or degenerate capture cannot be overlooked, regardless of
/// `--format`. stdout still carries only the structured status payload.
fn emit_stop_warning_banner(warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    eprintln!("===================== WARNING =====================");
    for warning in warnings {
        eprintln!("- {warning}");
    }
    eprintln!("==================================================");
}

fn meet_export(id: Option<String>, format: MeetExportFormat) -> Result<(), ComlinkError> {
    let resolved = config::load(CliConfigOverrides::default())?;
    let ctx = MeetContext::new(resolved, None);
    let kind = match format {
        MeetExportFormat::Json => meet::MeetingExportKind::Json,
        MeetExportFormat::Md => meet::MeetingExportKind::Markdown,
    };
    let export = meet_service::export(&ctx, id, kind)?;
    println!("{export}");
    Ok(())
}

fn llm_privacy_status(config: &config::LocalLlmConfig) -> String {
    if !config.enabled {
        return "disabled; local LLM rewrite is opt-in".to_string();
    }
    let model = config.model.as_deref().unwrap_or("<none>");
    format!(
        "enabled; provider={}; endpoint={}; model={}; policy={}",
        config.provider.as_str(),
        config.endpoint,
        model,
        crate::llm::LLM_CONTEXT_POLICY
    )
}

fn maybe_save_transcript(
    resolved: &config::ResolvedConfig,
    transcript: &mut output::TranscriptOutput,
    save: bool,
    audio_path: Option<&std::path::Path>,
) -> Result<(), ComlinkError> {
    if !save {
        return Ok(());
    }
    if !resolved.config.history_enabled {
        eprintln!("History is disabled; transcript was not saved.");
        return Ok(());
    }
    let id = storage::save_transcript(
        &resolved.paths,
        &resolved.config.retention,
        transcript,
        audio_path,
    )?;
    transcript.history_session_id = Some(id.clone());
    eprintln!("Saved history session: {id}");
    Ok(())
}

fn print_history_list(
    sessions: &[StoredSessionSummary],
    format: ConfigFormat,
) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(sessions)?),
        ConfigFormat::Text => {
            for session in sessions {
                println!(
                    "{} {} {} {}ms transcript={} audio={}",
                    session.id,
                    session.mode,
                    session.source_path,
                    session.duration_ms,
                    session.has_transcript,
                    session.has_audio
                );
            }
        }
    }
    Ok(())
}

fn print_history_session(
    session: &StoredSession,
    format: OutputFormat,
) -> Result<(), ComlinkError> {
    let output = StoredSessionOutput::from(session);
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&output)?),
        OutputFormat::Jsonl => {
            println!(
                "{}",
                serde_json::to_string(&StoredSessionJsonlRecord {
                    record_type: "transcript",
                    session: &output,
                })?
            );
        }
        OutputFormat::Md => println!("{}", render_stored_session_markdown(&output)),
        OutputFormat::Text => {
            println!("id: {}", session.id);
            println!("created_at_ms: {}", session.created_at_ms);
            println!("mode: {}", session.mode);
            println!("engine: {}", session.engine);
            println!("model: {}", session.model);
            println!("source: {}", session.source.path);
            println!("duration_ms: {}", session.duration_ms);
            if let Some(text) = &session.final_text {
                println!("final_text: {text}");
            } else {
                println!("final_text: <not retained>");
            }
        }
    }
    Ok(())
}

fn print_prune_result(result: &PruneResult, format: ConfigFormat) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(result)?),
        ConfigFormat::Text => println!(
            "deleted sessions={}, segments={}, audio_files={}",
            result.sessions_deleted, result.segments_deleted, result.audio_files_deleted
        ),
    }
    Ok(())
}

fn print_models(
    resolved: &config::ResolvedConfig,
    format: ConfigFormat,
) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resolved.config.models)?)
        }
        ConfigFormat::Text => {
            for model in &resolved.config.models {
                let marker = if model.selected { "*" } else { " " };
                println!("{marker} {} {}", model.name, model.path.display());
            }
        }
    }
    Ok(())
}

fn print_modes(config: &config::Config, format: ConfigFormat) -> Result<(), ComlinkError> {
    let modes = text::mode_registry(config);
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(&modes)?),
        ConfigFormat::Text => {
            for mode in modes {
                println!("{}: {}", mode.name, mode.description);
            }
        }
    }
    Ok(())
}

fn print_style_profiles(
    profiles: &[config::StyleProfile],
    format: ConfigFormat,
) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(profiles)?),
        ConfigFormat::Text => {
            for profile in profiles {
                println!(
                    "{}: {} ({} example(s))",
                    profile.name,
                    profile.summary,
                    profile.examples.len()
                );
            }
        }
    }
    Ok(())
}

fn print_processed_text(
    output: &ProcessedTextOutput,
    format: ConfigFormat,
) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(output)?),
        ConfigFormat::Text => println!("{}", output.final_text),
    }
    Ok(())
}

fn print_vocabulary(
    vocabulary: &[config::VocabularyEntry],
    format: ConfigFormat,
) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(vocabulary)?),
        ConfigFormat::Text => {
            for entry in vocabulary {
                println!("{} -> {}", entry.phrase, entry.replacement);
            }
        }
    }
    Ok(())
}

fn print_snippets(
    snippets: &[config::SnippetEntry],
    format: ConfigFormat,
) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(snippets)?),
        ConfigFormat::Text => {
            for entry in snippets {
                println!("{} -> {}", entry.trigger, entry.body.replace('\n', "\\n"));
            }
        }
    }
    Ok(())
}

fn decode_cli_newlines(text: &str) -> String {
    text.replace("\\n", "\n")
}

#[derive(Debug, Clone, Serialize)]
struct ProcessedTextOutput {
    raw_text: String,
    final_text: String,
    mode: String,
    processing_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct PrivacyAudit {
    history_enabled: bool,
    retention: config::RetentionConfig,
    config_file: String,
    database_file: String,
    selected_model: Option<String>,
    selected_model_exists: bool,
    asr: String,
    llm: String,
    system_audio: PrivacySystemAudio,
    /// Meeting audio the retention policy does not keep. `clean` is false
    /// while any is listed or the scan hit an error.
    meeting_audio: meet_service::MeetingAudioAudit,
    /// Local stdio MCP server posture (`comlink mcp`), beside `meeting_audio`.
    mcp: meet_service::McpPrivacy,
}

#[derive(Debug, Clone, Serialize)]
struct PrivacySystemAudio {
    strategy: String,
    available: bool,
    status: String,
    device_name: Option<String>,
    microphone_permission: String,
    routing_permission: String,
    raw_audio_retained: bool,
}

#[derive(Debug, Clone, Serialize)]
struct StoredSessionOutput {
    schema_version: String,
    session_id: String,
    created_at_ms: i64,
    text: Option<String>,
    raw_text: Option<String>,
    final_text: Option<String>,
    mode: String,
    copied: bool,
    engine: String,
    model: String,
    duration_ms: u64,
    segments: Vec<StoredSegment>,
    source: SourceMetadata,
    context: ContextMetadata,
    audio_path: Option<String>,
}

impl From<&StoredSession> for StoredSessionOutput {
    fn from(session: &StoredSession) -> Self {
        Self {
            schema_version: session.schema_version.clone(),
            session_id: session.id.clone(),
            created_at_ms: session.created_at_ms,
            text: session.final_text.clone(),
            raw_text: session.raw_text.clone(),
            final_text: session.final_text.clone(),
            mode: session.mode.clone(),
            copied: session.copied,
            engine: session.engine.clone(),
            model: session.model.clone(),
            duration_ms: session.duration_ms,
            segments: session.segments.clone(),
            source: session.source.clone(),
            context: ContextMetadata::default(),
            audio_path: session.audio_path.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct StoredSessionJsonlRecord<'a> {
    record_type: &'static str,
    #[serde(flatten)]
    session: &'a StoredSessionOutput,
}

fn render_stored_session_markdown(session: &StoredSessionOutput) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Comlink Transcript\n\n");
    markdown.push_str("## Metadata\n\n");
    markdown.push_str(&format!("- **Schema:** {}\n", session.schema_version));
    markdown.push_str(&format!("- **Session:** {}\n", session.session_id));
    markdown.push_str(&format!("- **Created:** {}\n", session.created_at_ms));
    markdown.push_str(&format!("- **Mode:** {}\n", session.mode));
    markdown.push_str(&format!("- **Engine:** {}\n", session.engine));
    markdown.push_str(&format!("- **Model:** {}\n", session.model));
    markdown.push_str(&format!("- **Duration:** {} ms\n", session.duration_ms));
    markdown.push_str(&format!("- **Source:** {}\n", session.source.path));
    markdown.push_str(&format!(
        "- **Source audio:** {} Hz, {} channel(s)\n",
        session.source.normalized_sample_rate_hz, session.source.normalized_channels
    ));
    markdown.push_str(&format!(
        "- **Context policy:** {}\n\n",
        session.context.policy
    ));

    markdown.push_str("## Final Text\n\n");
    markdown.push_str(session.final_text.as_deref().unwrap_or("<not retained>"));
    markdown.push_str("\n\n## Raw Text\n\n");
    markdown.push_str(session.raw_text.as_deref().unwrap_or("<not retained>"));

    if !session.segments.is_empty() {
        markdown.push_str("\n\n## Segments\n\n");
        for (index, segment) in session.segments.iter().enumerate() {
            markdown.push_str(&format!(
                "- {}: {}-{} ms: {}\n",
                index,
                segment.start_ms,
                segment.end_ms,
                segment.text.as_deref().unwrap_or("<not retained>")
            ));
        }
    }

    markdown.trim_end().to_string()
}

fn print_privacy_audit(audit: &PrivacyAudit, format: ConfigFormat) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(audit)?),
        ConfigFormat::Text => {
            println!("history_enabled: {}", audit.history_enabled);
            println!(
                "retention: metadata={}, transcripts={}, audio={}",
                audit.retention.metadata, audit.retention.transcripts, audit.retention.audio
            );
            println!("config_file: {}", audit.config_file);
            println!("database_file: {}", audit.database_file);
            println!(
                "selected_model: {}",
                audit.selected_model.as_deref().unwrap_or("<none>")
            );
            println!("selected_model_exists: {}", audit.selected_model_exists);
            println!("asr: {}", audit.asr);
            println!("llm: {}", audit.llm);
            println!(
                "system_audio: strategy={} available={} status={} device={} raw_audio_retained={}",
                audit.system_audio.strategy,
                audit.system_audio.available,
                audit.system_audio.status,
                audit
                    .system_audio
                    .device_name
                    .as_deref()
                    .unwrap_or("<none>"),
                audit.system_audio.raw_audio_retained
            );
            println!(
                "system_audio_microphone_permission: {}",
                audit.system_audio.microphone_permission
            );
            println!(
                "system_audio_routing_permission: {}",
                audit.system_audio.routing_permission
            );
            println!(
                "meeting_audio: clean={} unretained_leftovers={} scan_errors={}",
                audit.meeting_audio.clean,
                audit.meeting_audio.unretained_leftovers.len(),
                audit.meeting_audio.scan_errors.len()
            );
            for leftover in &audit.meeting_audio.unretained_leftovers {
                println!(
                    "meeting_audio_leftover: session={} status={} chunk_files={} chunks_dir={} remedy=`{}` reason={}",
                    leftover.session_id,
                    leftover.status,
                    leftover.chunk_files,
                    leftover.chunks_dir,
                    leftover.remedy,
                    leftover.reason
                );
            }
            for scan_error in &audit.meeting_audio.scan_errors {
                println!(
                    "meeting_audio_scan_error: path={} reason={}",
                    scan_error.path, scan_error.reason
                );
            }
            println!(
                "mcp: transport={} network_listener={} allow_start={} transcripts_sent_to_calling_model={}",
                audit.mcp.transport,
                audit.mcp.network_listener,
                audit.mcp.allow_start,
                audit.mcp.transcripts_sent_to_calling_model
            );
        }
    }
    Ok(())
}

fn print_meet_start(status: &MeetStartStatus, format: ConfigFormat) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(status)?),
        ConfigFormat::Text => {
            println!("session_id: {}", status.session_id);
            println!("status: {}", status.status);
            println!("elapsed_ms: {}", status.elapsed_ms);
            println!("recorder_pid: {}", status.recorder_pid);
            println!("source_mode: {}", status.source.mode.as_str());
            for recorder in &status.recorders {
                println!(
                    "recorder: source={} pid={} device={} chunks_dir={} stderr={}",
                    recorder.source_label.as_str(),
                    recorder.pid,
                    recorder.device,
                    recorder.chunks_dir,
                    recorder.stderr_path
                );
            }
            println!("session_dir: {}", status.session_dir);
            println!("chunks_dir: {}", status.chunks_dir);
            println!("segments_jsonl: {}", status.segments_jsonl);
            println!("json_export: {}", status.json_export);
            println!("markdown_export: {}", status.markdown_export);
            println!(
                "inactivity_auto_stop: enabled={} ({})",
                status.inactivity_auto_stop.enabled, status.inactivity_auto_stop.reason
            );
            println!("{}", status.consent_reminder);
        }
    }
    Ok(())
}

fn print_meet_stop(status: &MeetStopStatus, format: ConfigFormat) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(status)?),
        ConfigFormat::Text => {
            println!("session_id: {}", status.session_id);
            println!("status: {}", status.status);
            println!("elapsed_ms: {}", status.elapsed_ms);
            println!("duration_ms: {}", status.duration_ms);
            println!("chunks_processed: {}", status.chunks_processed);
            println!("segment_count: {}", status.segment_count);
            println!("source_mode: {}", status.source.mode.as_str());
            for stream in &status.source.streams {
                println!("source_stream: {} {}", stream.label.as_str(), stream.device);
            }
            println!("segments_jsonl: {}", status.artifacts.segments_jsonl);
            println!("json_export: {}", status.artifacts.json_export);
            println!("markdown_export: {}", status.artifacts.markdown_export);
            println!(
                "retention: metadata={}, transcripts={}, audio={}",
                status.retention.metadata, status.retention.transcripts, status.retention.audio
            );
            println!(
                "segmenting: {} vad_available={}",
                status.segmenting.strategy, status.segmenting.vad_available
            );
            if let Some(level) = &status.audio_level {
                println!(
                    "audio_level: mean={:.1} dBFS, peak={:.1} dBFS, near_silent={}",
                    level.mean_dbfs, level.peak_dbfs, level.near_silent
                );
            }
            println!(
                "inactivity_auto_stop: enabled={} ({})",
                status.inactivity_auto_stop.enabled, status.inactivity_auto_stop.reason
            );
            for warning in &status.warnings {
                println!("warning: {warning}");
            }
        }
    }
    Ok(())
}

fn print_meet_detached(
    status: &MeetDetachedStatus,
    format: ConfigFormat,
) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(status)?),
        ConfigFormat::Text => {
            println!("session_id: {}", status.session_id);
            println!("status: {}", status.status);
            println!("elapsed_ms: {}", status.elapsed_ms);
            println!(
                "preliminary_duration_ms: {}",
                status.preliminary_duration_ms
            );
            println!("chunk_count: {}", status.chunk_count);
            println!("finalizer_pid: {}", status.finalizer_pid);
            println!("finalize_log: {}", status.finalize_log);
            println!("segments_jsonl: {}", status.artifacts.segments_jsonl);
            println!("json_export: {}", status.artifacts.json_export);
            println!("markdown_export: {}", status.artifacts.markdown_export);
        }
    }
    Ok(())
}

fn print_meet_status(report: &MeetStatusReport, format: ConfigFormat) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(report)?),
        ConfigFormat::Text => {
            println!(
                "session_id: {}",
                report.session_id.as_deref().unwrap_or("<none>")
            );
            println!("status: {}", report.status);
            if let Some(elapsed_ms) = report.elapsed_ms {
                println!("elapsed_ms: {elapsed_ms}");
            }
            for recorder in &report.recorders {
                println!(
                    "recorder: source={} pid={} alive={} device={}",
                    recorder.source_label.as_str(),
                    recorder
                        .pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                    recorder.alive,
                    recorder.device
                );
            }
            if let Some(finalizer) = &report.finalizer {
                println!("finalizer: pid={} alive={}", finalizer.pid, finalizer.alive);
            }
            if report.session_id.is_some() {
                println!("chunk_count: {}", report.chunk_count);
            }
            if let Some(level) = &report.audio_level {
                println!(
                    "audio_level: mean={:.1} dBFS, peak={:.1} dBFS, near_silent={}",
                    level.mean_dbfs, level.peak_dbfs, level.near_silent
                );
            }
            println!("stale: {}", report.stale);
            if let Some(error) = &report.error {
                println!("error: {error}");
            }
            if let Some(log) = &report.finalize_log {
                println!("finalize_log: {log}");
            }
            for warning in &report.warnings {
                println!("warning: {warning}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_requested_mode_rejects_unknown_mode_before_asr() {
        let error = validate_requested_mode(&config::Config::default(), "missing").unwrap_err();

        assert!(matches!(error, ComlinkError::ModeNotFound(mode) if mode == "missing"));
    }

    #[test]
    fn validate_requested_mode_accepts_configured_mode() {
        let mut config = config::Config::default();
        config.modes.push(config::ModeEntry {
            name: "prompt".to_string(),
            description: None,
            deterministic_mode: Some("memo".to_string()),
            llm_instruction: Some("Rewrite as a prompt".to_string()),
            style_profile: None,
        });

        validate_requested_mode(&config, "prompt").unwrap();
    }
}
