use std::env;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{
    asr::{AsrEngine, SourceMetadata, WhisperCppEngine},
    audio, clipboard,
    config::{self, CliConfigOverrides, ConfigFormat},
    deps, doctor,
    error::ComlinkError,
    output::{self, OutputFormat},
    record,
    storage::{self, PruneResult, StoredSession, StoredSessionSummary},
    text::{self, TextMode, TextRules},
};

const DEFAULT_RECORD_DEVICE: &str = ":0";
const DEFAULT_MIN_RECORDING_MS: u64 = 300;

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
    Doctor,

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
        #[arg(long, value_enum, default_value = "raw")]
        mode: TextMode,

        /// whisper.cpp ggml model path. Defaults to COMLINK_WHISPER_MODEL.
        #[arg(long)]
        model: Option<PathBuf>,

        /// Save this transcript to local history when history is enabled.
        #[arg(long)]
        save: bool,
    },

    /// Record a short microphone memo and transcribe it locally.
    Record {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,

        /// Text processing mode.
        #[arg(long, value_enum, default_value = "memo")]
        mode: TextMode,

        /// Copy final text to the macOS clipboard.
        #[arg(long)]
        copy: bool,

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
        format: ConfigFormat,
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
    /// List built-in deterministic modes.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },

    /// Process plain text through a mode without ASR.
    Apply {
        /// Text processing mode.
        #[arg(long, value_enum)]
        mode: TextMode,

        /// Text to process.
        #[arg(long)]
        text: String,

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

pub fn run() -> Result<(), ComlinkError> {
    let cli = Cli::parse();

    match cli.command {
        Command::Doctor => {
            let healthy = doctor::run();
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
        Command::Vocab { command } => run_vocab(command),
        Command::Snippets { command } => run_snippets(command),
        Command::Privacy { command } => run_privacy(command),
        Command::Transcribe {
            file,
            format,
            mode,
            model,
            save,
        } => transcribe(file, format, mode, model, save),
        Command::Record {
            format,
            mode,
            copy,
            model,
            min_duration_ms,
            device,
            save,
        } => record_memo(format, mode, copy, model, min_duration_ms, device, save),
    }
}

fn transcribe(
    file: PathBuf,
    format: OutputFormat,
    mode: TextMode,
    model: Option<PathBuf>,
    save: bool,
) -> Result<(), ComlinkError> {
    let resolved = config::load(CliConfigOverrides { model })?;
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
    let mut transcript =
        output::TranscriptOutput::from_transcript(transcript, mode, false, &resolved.config);
    maybe_save_transcript(&resolved, &mut transcript, save, Some(&normalized.path))?;
    output::print_transcript(&transcript, format)
}

fn record_memo(
    format: OutputFormat,
    mode: TextMode,
    copy: bool,
    model: Option<PathBuf>,
    min_duration_ms: u64,
    device: Option<String>,
    save: bool,
) -> Result<(), ComlinkError> {
    let resolved = config::load(CliConfigOverrides { model })?;
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
    let mut transcript =
        output::TranscriptOutput::from_transcript(transcript, mode, copy, &resolved.config);
    let stop_to_final_ms = captured.stopped_at.elapsed().as_millis();

    if copy {
        clipboard::copy_text(&transcript.final_text)?;
        eprintln!("Copied final text to clipboard.");
    }

    maybe_save_transcript(&resolved, &mut transcript, save, Some(&captured.path))?;
    eprintln!("Stop-to-final latency: {} ms.", stop_to_final_ms);
    output::print_transcript(&transcript, format)
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
    let mut resolved = config::load(CliConfigOverrides::default())?;
    match command {
        ModelsCommand::List { format } => print_models(&resolved, format),
        ModelsCommand::Select { name, path } => {
            if !path.is_file() {
                return Err(ComlinkError::ModelPathMissing(path));
            }
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
        ModesCommand::List { format } => print_modes(format),
        ModesCommand::Apply { mode, text, format } => {
            let resolved = config::load(CliConfigOverrides::default())?;
            let final_text = mode.process(
                &text,
                TextRules {
                    vocabulary: &resolved.config.vocabulary,
                    snippets: &resolved.config.snippets,
                },
            );
            let output = ProcessedTextOutput {
                raw_text: text,
                final_text,
                mode,
                processing_steps: mode
                    .processing_steps()
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            };
            print_processed_text(&output, format)
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
                llm: "disabled; no cloud endpoint configured".to_string(),
            };
            print_privacy_audit(&audit, format)
        }
    }
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
    format: ConfigFormat,
) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(session)?),
        ConfigFormat::Text => {
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

fn print_modes(format: ConfigFormat) -> Result<(), ComlinkError> {
    let modes = text::mode_registry();
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
    mode: TextMode,
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
        }
    }
    Ok(())
}
