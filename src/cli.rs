use std::{
    env, fs,
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
    output::{self, ContextMetadata, OutputFormat},
    record,
    storage::{self, PruneResult, StoredSegment, StoredSession, StoredSessionSummary},
    system_audio, text,
};

const DEFAULT_RECORD_DEVICE: &str = ":0";
const DEFAULT_MIN_RECORDING_MS: u64 = 300;
const DEFAULT_MEETING_CHUNK_SECONDS: u64 = 300;
const DEFAULT_MEETING_STOP_TIMEOUT_SECONDS: u64 = 15;

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

        /// FFmpeg AVFoundation input device. Defaults to COMLINK_RECORD_DEVICE or :0.
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
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Show resolved config, paths, and source precedence.
    Show {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
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

        /// FFmpeg AVFoundation input device. Defaults to COMLINK_RECORD_DEVICE or :0.
        #[arg(long)]
        device: Option<String>,

        /// Meeting capture source. Defaults to Phase 7-compatible mic-only.
        #[arg(long, value_parser = ["mic-only", "system-only", "mic-plus-system"], default_value = "mic-only")]
        source: String,

        /// BlackHole FFmpeg AVFoundation audio input. Defaults to COMLINK_SYSTEM_AUDIO_DEVICE or detected BlackHole.
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
        Command::Doctor { format } => {
            let healthy = doctor::run(format)?;
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
    let device = device
        .or_else(|| env::var("COMLINK_RECORD_DEVICE").ok())
        .unwrap_or_else(|| DEFAULT_RECORD_DEVICE.to_string());

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

    let engine = WhisperCppEngine {
        binary: runtime.whisper_cpp,
        model: runtime.whisper_model,
    };
    let source = SourceMetadata {
        path: "microphone".to_string(),
        normalized_sample_rate_hz: captured.sample_rate_hz,
        normalized_channels: captured.channels,
    };

    let transcript = engine.transcribe(&captured.path, source, captured.duration_ms)?;
    let mut transcript = output::TranscriptOutput::from_transcript(
        transcript,
        mode,
        copy,
        &resolved.config,
        no_llm,
    )?;
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
    text::resolve_mode(config, mode)
        .map(|_| ())
        .ok_or_else(|| ComlinkError::ModeNotFound(mode.to_string()))
}

fn run_config(command: ConfigCommand) -> Result<(), ComlinkError> {
    match command {
        ConfigCommand::Show { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            config::print(&resolved, format)
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
            let mut resolved = config::load_persistent()?;
            let path = path.canonicalize()?;
            config::select_model(&mut resolved, &name, path);
            config::save(&resolved.paths, &resolved.config)?;
            println!(
                "selected model: {}",
                resolved
                    .config
                    .selected_model
                    .as_deref()
                    .unwrap_or("<none>")
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
            let mut resolved = config::load_persistent()?;
            config::upsert_mode(
                &mut resolved.config,
                name.clone(),
                description,
                Some(deterministic_mode),
                Some(instruction),
                style_profile,
            );
            config::save(&resolved.paths, &resolved.config)?;
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
            let mut resolved = config::load_persistent()?;
            if !config::remove_mode(&mut resolved.config, &name) {
                return Err(ComlinkError::NotFound { kind: "mode", name });
            }
            config::save(&resolved.paths, &resolved.config)?;
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
            let mut resolved = config::load_persistent()?;
            config::upsert_style_profile(&mut resolved.config, profile);
            config::save(&resolved.paths, &resolved.config)?;
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
            let mut resolved = config::load_persistent()?;
            config::upsert_vocabulary(&mut resolved.config, phrase.clone(), replacement.clone());
            config::save(&resolved.paths, &resolved.config)?;
            println!("{phrase} -> {replacement}");
            Ok(())
        }
        VocabCommand::List { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            print_vocabulary(&resolved.config.vocabulary, format)
        }
        VocabCommand::Remove { phrase } => {
            let mut resolved = config::load_persistent()?;
            if !config::remove_vocabulary(&mut resolved.config, &phrase) {
                return Err(ComlinkError::NotFound {
                    kind: "vocabulary",
                    name: phrase,
                });
            }
            config::save(&resolved.paths, &resolved.config)?;
            println!("removed vocabulary phrase");
            Ok(())
        }
    }
}

fn run_snippets(command: SnippetsCommand) -> Result<(), ComlinkError> {
    match command {
        SnippetsCommand::Add { trigger, body } => {
            let mut resolved = config::load_persistent()?;
            let body = decode_cli_newlines(&body);
            config::upsert_snippet(&mut resolved.config, trigger.clone(), body);
            config::save(&resolved.paths, &resolved.config)?;
            println!("saved snippet: {trigger}");
            Ok(())
        }
        SnippetsCommand::List { format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            print_snippets(&resolved.config.snippets, format)
        }
        SnippetsCommand::Remove { trigger } => {
            let mut resolved = config::load_persistent()?;
            if !config::remove_snippet(&mut resolved.config, &trigger) {
                return Err(ComlinkError::NotFound {
                    kind: "snippet",
                    name: trigger,
                });
            }
            config::save(&resolved.paths, &resolved.config)?;
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
        } => meet_stop(MeetStopOptions {
            id,
            format,
            wait_timeout_seconds,
        }),
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
    validate_requested_mode(&resolved.config, mode)?;
    let model_path =
        config::selected_model_path(&resolved.config).ok_or(ComlinkError::ModelMissing)?;
    let runtime = deps::runtime_from_model_path(model_path.clone())?;
    let device = device
        .or_else(|| env::var("COMLINK_RECORD_DEVICE").ok())
        .unwrap_or_else(|| DEFAULT_RECORD_DEVICE.to_string());
    let source_mode =
        meet::MeetSourceMode::parse(source).ok_or_else(|| ComlinkError::InvalidConfigValue {
            name: "meet start --source",
            value: source.to_string(),
        })?;
    let source_metadata =
        build_meeting_source_metadata(source_mode, &device, system_device, &runtime.ffmpeg)?;
    let chunk_seconds = chunk_seconds.max(1);
    let store = meet::FileMeetingStore::new(&resolved.paths);

    if let Some(active_id) = store.active_session_id()? {
        match store.read_session(&active_id) {
            Ok(mut session) if session.status == meet::MeetingStatus::Recording => {
                if session_recorder_is_verified_running(&session) {
                    return Err(ComlinkError::MeetingAlreadyActive(active_id));
                }
                reclaim_inactive_recording_session(&store, &mut session)?;
            }
            _ => store.clear_active_if_matches(&active_id)?,
        }
    }

    let paths = store.paths_for_new_session();
    let source_metadata = source_metadata.with_session_paths(
        &paths.chunks_dir,
        &paths.recorder_stderr_path,
        source_mode == meet::MeetSourceMode::MicOnly,
    );
    let mut session = meet::new_recording_session(meet::NewMeetingSession {
        session_id: paths.session_id,
        mode: mode.to_string(),
        no_llm,
        device: device.clone(),
        source: source_metadata,
        chunk_duration_ms: chunk_seconds.saturating_mul(1000),
        model: runtime.whisper_model.display().to_string(),
        model_path: runtime.whisper_model.display().to_string(),
        retention: meet::MeetingRetentionPolicy::from(&resolved.config.retention),
        session_dir: paths.session_dir,
        chunks_dir: paths.chunks_dir,
        recorder_stderr_path: paths.recorder_stderr_path,
        segments_jsonl_path: paths.segments_jsonl_path,
        json_export_path: paths.json_export_path,
        markdown_export_path: paths.markdown_export_path,
    });

    store.create_session(&session)?;
    let captures = match start_session_recorders(&runtime.ffmpeg, &session, chunk_seconds) {
        Ok(captures) => captures,
        Err(error) => {
            store.clear_active_if_matches(&session.session_id)?;
            return Err(error);
        }
    };
    apply_started_recorders(&mut session, captures);
    store.save_session(&session)?;

    let consent_reminder =
        "Consent reminder: confirm everyone present knows this meeting is being recorded and transcribed.";
    eprintln!("{consent_reminder}");
    print_meet_start(
        &MeetStartStatus {
            schema_version: meet::MEETING_SCHEMA_VERSION,
            session_id: session.session_id.clone(),
            status: meet::MeetingStatus::Recording.as_str(),
            elapsed_ms: elapsed_since(session.started_at_ms),
            recorder_pid: session.recorder_pid.unwrap_or_default(),
            recorders: session_recorders_status(&session),
            source: session.source.clone(),
            session_dir: session.session_dir.clone(),
            chunks_dir: session.chunks_dir.clone(),
            segments_jsonl: session.segments_jsonl_path.clone(),
            json_export: session.json_export_path.clone(),
            markdown_export: session.markdown_export_path.clone(),
            consent_reminder,
            inactivity_auto_stop: session.inactivity_auto_stop.clone(),
        },
        format,
    )
}

fn build_meeting_source_metadata(
    mode: meet::MeetSourceMode,
    mic_device: &str,
    system_device: Option<String>,
    ffmpeg: &Path,
) -> Result<meet::MeetingSourceMetadata, ComlinkError> {
    let system_device = if matches!(
        mode,
        meet::MeetSourceMode::SystemOnly | meet::MeetSourceMode::MicPlusSystem
    ) {
        Some(resolve_system_audio_device(system_device, ffmpeg)?)
    } else {
        None
    };

    if mode == meet::MeetSourceMode::MicPlusSystem
        && system_device
            .as_deref()
            .is_some_and(|device| same_audio_device(device, mic_device))
    {
        return Ok(meet::MeetingSourceMetadata::new(
            mode,
            vec![meet::MeetingSourceStream {
                label: meet::MeetingSourceLabel::Mixed,
                device: mic_device.to_string(),
                chunks_dir: None,
                recorder_stderr_path: None,
                recorder_pid: None,
                recorder_identity: None,
            }],
        ));
    }

    let streams = match mode {
        meet::MeetSourceMode::MicOnly => vec![meet::MeetingSourceStream {
            label: meet::MeetingSourceLabel::UserMic,
            device: mic_device.to_string(),
            chunks_dir: None,
            recorder_stderr_path: None,
            recorder_pid: None,
            recorder_identity: None,
        }],
        meet::MeetSourceMode::SystemOnly => vec![meet::MeetingSourceStream {
            label: meet::MeetingSourceLabel::SystemAudio,
            device: system_device.unwrap(),
            chunks_dir: None,
            recorder_stderr_path: None,
            recorder_pid: None,
            recorder_identity: None,
        }],
        meet::MeetSourceMode::MicPlusSystem => vec![
            meet::MeetingSourceStream {
                label: meet::MeetingSourceLabel::UserMic,
                device: mic_device.to_string(),
                chunks_dir: None,
                recorder_stderr_path: None,
                recorder_pid: None,
                recorder_identity: None,
            },
            meet::MeetingSourceStream {
                label: meet::MeetingSourceLabel::SystemAudio,
                device: system_device.unwrap(),
                chunks_dir: None,
                recorder_stderr_path: None,
                recorder_pid: None,
                recorder_identity: None,
            },
        ],
    };

    Ok(meet::MeetingSourceMetadata::new(mode, streams))
}

fn resolve_system_audio_device(
    system_device: Option<String>,
    ffmpeg: &Path,
) -> Result<String, ComlinkError> {
    if let Some(device) = system_device
        .or_else(|| env::var("COMLINK_SYSTEM_AUDIO_DEVICE").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        if device.to_ascii_lowercase().contains("blackhole") {
            return Ok(device.trim_start_matches(':').to_string());
        }

        return Err(ComlinkError::AudioCaptureFailed(format!(
            "system-audio capture requires a BlackHole input device; got `{device}`. Install BlackHole 2ch or set COMLINK_SYSTEM_AUDIO_DEVICE to the exact BlackHole AVFoundation input name."
        )));
    }

    let probe = system_audio::RealSystemAudioProbe::from_ffmpeg(ffmpeg.to_path_buf());
    system_audio::resolve_capture_plan(&probe)
        .map(|plan| plan.device_name)
        .map_err(ComlinkError::AudioCaptureFailed)
}

fn same_audio_device(left: &str, right: &str) -> bool {
    left.trim_start_matches(':')
        .eq_ignore_ascii_case(right.trim_start_matches(':'))
}

fn start_session_recorders(
    ffmpeg: &Path,
    session: &meet::MeetingSessionState,
    chunk_seconds: u64,
) -> Result<Vec<(meet::MeetingSourceLabel, record::SegmentedCapture)>, ComlinkError> {
    let mut captures: Vec<(meet::MeetingSourceLabel, record::SegmentedCapture)> =
        Vec::with_capacity(session.source.streams.len());
    for stream in &session.source.streams {
        let chunks_dir = stream.chunks_dir.as_deref().ok_or_else(|| {
            ComlinkError::AudioCaptureFailed("missing stream chunks_dir".to_string())
        })?;
        let stderr_path = stream.recorder_stderr_path.as_deref().ok_or_else(|| {
            ComlinkError::AudioCaptureFailed("missing stream recorder stderr path".to_string())
        })?;
        let device = capture_device_for_stream(stream);
        let capture = match record::start_segmented_capture(record::SegmentedCaptureOptions {
            ffmpeg,
            device: &device,
            chunks_dir: Path::new(chunks_dir),
            stderr_path: Path::new(stderr_path),
            chunk_duration: Duration::from_secs(chunk_seconds),
        }) {
            Ok(capture) => capture,
            Err(error) => {
                for (_, capture) in &captures {
                    let _ = record::stop_segmented_capture(
                        &capture.identity,
                        Duration::from_secs(DEFAULT_MEETING_STOP_TIMEOUT_SECONDS),
                    );
                }
                return Err(error);
            }
        };
        captures.push((stream.label, capture));
    }
    Ok(captures)
}

fn capture_device_for_stream(stream: &meet::MeetingSourceStream) -> String {
    match stream.label {
        meet::MeetingSourceLabel::SystemAudio => {
            system_audio::avfoundation_audio_input(&stream.device)
        }
        meet::MeetingSourceLabel::Mixed
            if stream.device.to_ascii_lowercase().contains("blackhole") =>
        {
            system_audio::avfoundation_audio_input(&stream.device)
        }
        _ => stream.device.clone(),
    }
}

fn apply_started_recorders(
    session: &mut meet::MeetingSessionState,
    captures: Vec<(meet::MeetingSourceLabel, record::SegmentedCapture)>,
) {
    for (label, capture) in captures {
        if session.recorder_pid.is_none() {
            session.recorder_pid = Some(capture.pid);
            session.recorder_identity = Some(capture.identity.clone());
        }
        if let Some(stream) = session
            .source
            .streams
            .iter_mut()
            .find(|stream| stream.label == label)
        {
            stream.recorder_pid = Some(capture.pid);
            stream.recorder_identity = Some(capture.identity);
        }
    }
}

fn session_recorders_status(session: &meet::MeetingSessionState) -> Vec<MeetRecorderStatus> {
    session
        .source
        .streams
        .iter()
        .filter_map(|stream| {
            Some(MeetRecorderStatus {
                source_label: stream.label,
                device: stream.device.clone(),
                pid: stream.recorder_pid?,
                chunks_dir: stream
                    .chunks_dir
                    .clone()
                    .unwrap_or_else(|| session.chunks_dir.clone()),
                stderr_path: stream
                    .recorder_stderr_path
                    .clone()
                    .unwrap_or_else(|| session.recorder_stderr_path.clone()),
            })
        })
        .collect()
}

fn session_recorder_identities(
    session: &meet::MeetingSessionState,
) -> Vec<record::SegmentedCaptureIdentity> {
    let identities = session
        .source
        .streams
        .iter()
        .filter_map(|stream| {
            if let Some(identity) = &stream.recorder_identity {
                return Some(identity.clone());
            }
            stream.recorder_pid.map(|pid| {
                let chunks_dir = stream
                    .chunks_dir
                    .as_deref()
                    .unwrap_or(session.chunks_dir.as_str());
                let output_pattern = record::chunk_output_pattern(Path::new(chunks_dir));
                record::SegmentedCaptureIdentity::new(pid, &output_pattern)
            })
        })
        .collect::<Vec<_>>();

    if !identities.is_empty() {
        return identities;
    }

    session
        .recorder_pid
        .map(|pid| {
            let output_pattern = record::chunk_output_pattern(Path::new(&session.chunks_dir));
            record::SegmentedCaptureIdentity::new(pid, &output_pattern)
        })
        .into_iter()
        .collect()
}

fn session_recorder_is_verified_running(session: &meet::MeetingSessionState) -> bool {
    let identities = session_recorder_identities(session);
    !identities.is_empty() && identities.iter().all(record::segmented_capture_is_running)
}

fn reclaim_inactive_recording_session(
    store: &meet::FileMeetingStore,
    session: &mut meet::MeetingSessionState,
) -> Result<(), ComlinkError> {
    let duration_ms = store
        .discover_chunks(session, |path| audio::probe_duration_ms(path, None))
        .ok()
        .map(|chunks| meeting_duration_ms(&chunks))
        .unwrap_or_default();
    session.mark_stopped(meet::now_ms(), duration_ms, 0);
    store.save_session(session)?;
    store.clear_active_if_matches(&session.session_id)
}

fn stop_session_recorder(
    session: &meet::MeetingSessionState,
    timeout: Duration,
) -> Result<bool, ComlinkError> {
    let identities = session_recorder_identities(session);
    if identities.is_empty() {
        return Ok(false);
    }
    let mut stopped_any = false;
    for identity in identities {
        stopped_any |= record::stop_segmented_capture(&identity, timeout)?;
    }
    Ok(stopped_any)
}

struct MeetStopOptions {
    id: Option<String>,
    format: ConfigFormat,
    wait_timeout_seconds: u64,
}

fn meet_stop(options: MeetStopOptions) -> Result<(), ComlinkError> {
    let MeetStopOptions {
        id,
        format,
        wait_timeout_seconds,
    } = options;
    let resolved = config::load(CliConfigOverrides::default())?;
    let store = meet::FileMeetingStore::new(&resolved.paths);
    let id = match id {
        Some(id) => id,
        None => store
            .active_session_id()?
            .ok_or(ComlinkError::MeetingNoActiveSession)?,
    };
    let mut session = store.read_session(&id)?;
    if session.status != meet::MeetingStatus::Recording {
        return Err(ComlinkError::MeetingNotRecording(id));
    }

    eprintln!("Stopping meeting recording: {}", session.session_id);
    let stopped_at_ms = meet::now_ms();
    stop_session_recorder(&session, Duration::from_secs(wait_timeout_seconds.max(1)))?;
    std::thread::sleep(Duration::from_millis(250));

    let preliminary_chunks =
        store.discover_chunks(&session, |path| audio::probe_duration_ms(path, None))?;
    let preliminary_duration_ms = meeting_duration_ms(&preliminary_chunks);
    session.mark_stopped(stopped_at_ms, preliminary_duration_ms, 0);
    store.save_session(&session)?;
    store.clear_active_if_matches(&session.session_id)?;

    if preliminary_chunks.is_empty() {
        return Err(ComlinkError::AudioCaptureFailed(format!(
            "meeting recording produced no chunk files; see {}",
            session.recorder_stderr_path
        )));
    }

    let runtime = deps::runtime_from_model_path(PathBuf::from(&session.model_path))?;
    let chunks = store.discover_chunks(&session, |path| {
        audio::probe_duration_ms(path, runtime.ffprobe.as_deref())
    })?;
    if chunks.is_empty() {
        return Err(ComlinkError::AudioCaptureFailed(format!(
            "meeting recording produced no chunk files; see {}",
            session.recorder_stderr_path
        )));
    }

    let engine = WhisperCppEngine {
        binary: runtime.whisper_cpp,
        model: runtime.whisper_model,
    };
    let mut chunk_transcripts = Vec::with_capacity(chunks.len());
    for chunk in &chunks {
        let source = SourceMetadata {
            path: chunk.path.display().to_string(),
            normalized_sample_rate_hz: session.sample_rate_hz,
            normalized_channels: session.channels,
        };
        let transcript = match engine.transcribe(&chunk.path, source, chunk.duration_ms) {
            Ok(transcript) => transcript,
            Err(ComlinkError::EmptyTranscript) => continue,
            Err(error) => return Err(error),
        };
        chunk_transcripts.push(meet::ChunkTranscript {
            chunk_index: chunk.index,
            chunk_path: chunk.path.clone(),
            source_label: chunk.source_label,
            source_device: chunk.source_device.clone(),
            chunk_start_ms: chunk.start_ms,
            chunk_duration_ms: chunk.duration_ms,
            transcript,
        });
    }

    let segment_result = meet::build_segments_from_chunk_transcripts(&chunk_transcripts);
    let raw_text = meet::raw_text_from_segments(&segment_result.segments);
    let processed =
        output::process_text(&raw_text, &session.mode, &resolved.config, session.no_llm)?;
    let duration_ms = meeting_duration_ms(&chunks);
    session.mark_stopped(stopped_at_ms, duration_ms, segment_result.segments.len());
    let export = meet::build_export(
        &session,
        &segment_result.segments,
        &raw_text,
        &processed,
        segment_result.segmenting,
    );

    store.save_session(&session)?;
    store.write_segments_jsonl(&export)?;
    store.write_exports(&export)?;
    if !session.retention.audio {
        store.delete_chunks(&session)?;
    }

    print_meet_stop(
        &MeetStopStatus {
            schema_version: meet::MEETING_SCHEMA_VERSION,
            session_id: session.session_id,
            status: meet::MeetingStatus::Stopped.as_str(),
            elapsed_ms: elapsed_since(session.started_at_ms),
            duration_ms,
            chunks_processed: chunks.len(),
            segment_count: export.session.segment_count,
            source: export.source.clone(),
            retention: export.retention,
            segmenting: export.segmenting,
            artifacts: export.artifacts,
            inactivity_auto_stop: session.inactivity_auto_stop,
        },
        format,
    )
}

fn meeting_duration_ms(chunks: &[meet::MeetingChunk]) -> u64 {
    chunks
        .iter()
        .map(meet::MeetingChunk::end_ms)
        .max()
        .unwrap_or_default()
}

fn meet_export(id: Option<String>, format: MeetExportFormat) -> Result<(), ComlinkError> {
    let resolved = config::load(CliConfigOverrides::default())?;
    let store = meet::FileMeetingStore::new(&resolved.paths);
    let id = match id {
        Some(id) => id,
        None => {
            if let Some(id) = store.latest_stopped_session_id()? {
                id
            } else if let Some(active_id) = store.active_session_id()? {
                return Err(ComlinkError::MeetingNotStopped(active_id));
            } else {
                return Err(ComlinkError::MeetingNoActiveSession);
            }
        }
    };
    let kind = match format {
        MeetExportFormat::Json => meet::MeetingExportKind::Json,
        MeetExportFormat::Md => meet::MeetingExportKind::Markdown,
    };
    let export = store.read_export(&id, kind)?;
    println!("{export}");
    Ok(())
}

fn elapsed_since(started_at_ms: i64) -> u64 {
    meet::now_ms().saturating_sub(started_at_ms) as u64
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

#[derive(Debug, Serialize)]
struct MeetStartStatus<'a> {
    schema_version: &'a str,
    session_id: String,
    status: &'a str,
    elapsed_ms: u64,
    recorder_pid: u32,
    recorders: Vec<MeetRecorderStatus>,
    source: meet::MeetingSourceMetadata,
    session_dir: String,
    chunks_dir: String,
    segments_jsonl: String,
    json_export: String,
    markdown_export: String,
    consent_reminder: &'a str,
    inactivity_auto_stop: meet::InactivityAutoStop,
}

#[derive(Debug, Clone, Serialize)]
struct MeetRecorderStatus {
    source_label: meet::MeetingSourceLabel,
    device: String,
    pid: u32,
    chunks_dir: String,
    stderr_path: String,
}

#[derive(Debug, Serialize)]
struct MeetStopStatus<'a> {
    schema_version: &'a str,
    session_id: String,
    status: &'a str,
    elapsed_ms: u64,
    duration_ms: u64,
    chunks_processed: usize,
    segment_count: usize,
    source: meet::MeetingSourceMetadata,
    retention: meet::MeetingRetentionPolicy,
    segmenting: meet::MeetingSegmenting,
    artifacts: meet::MeetingArtifacts,
    inactivity_auto_stop: meet::InactivityAutoStop,
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
        }
    }
    Ok(())
}

fn print_meet_start(
    status: &MeetStartStatus<'_>,
    format: ConfigFormat,
) -> Result<(), ComlinkError> {
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

fn print_meet_stop(status: &MeetStopStatus<'_>, format: ConfigFormat) -> Result<(), ComlinkError> {
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
            println!(
                "inactivity_auto_stop: enabled={} ({})",
                status.inactivity_auto_stop.enabled, status.inactivity_auto_stop.reason
            );
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
