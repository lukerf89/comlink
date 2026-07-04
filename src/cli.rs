use std::env;
use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};

use crate::{
    asr::{AsrEngine, SourceMetadata, WhisperCppEngine},
    audio, clipboard, deps, doctor,
    error::ComlinkError,
    output::{self, OutputFormat},
    record,
    text::TextMode,
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
        Command::Transcribe {
            file,
            format,
            mode,
            model,
        } => transcribe(file, format, mode, model),
        Command::Record {
            format,
            mode,
            copy,
            model,
            min_duration_ms,
            device,
        } => record_memo(format, mode, copy, model, min_duration_ms, device),
    }
}

fn transcribe(
    file: PathBuf,
    format: OutputFormat,
    mode: TextMode,
    model: Option<PathBuf>,
) -> Result<(), ComlinkError> {
    let runtime = deps::runtime_from_env(model)?;
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
    let transcript = output::TranscriptOutput::from_transcript(transcript, mode, false);
    output::print_transcript(&transcript, format)
}

fn record_memo(
    format: OutputFormat,
    mode: TextMode,
    copy: bool,
    model: Option<PathBuf>,
    min_duration_ms: u64,
    device: Option<String>,
) -> Result<(), ComlinkError> {
    let runtime = deps::runtime_from_env(model)?;
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
        return Err(ComlinkError::EmptyTranscript);
    }

    let stop_to_final = Instant::now();
    let normalized =
        audio::normalize_to_wav(&captured.path, &runtime.ffmpeg, runtime.ffprobe.as_deref())?;
    if normalized.duration_ms < min_duration_ms {
        return Err(ComlinkError::EmptyTranscript);
    }

    let engine = WhisperCppEngine {
        binary: runtime.whisper_cpp,
        model: runtime.whisper_model,
    };
    let source = SourceMetadata {
        path: "microphone".to_string(),
        normalized_sample_rate_hz: normalized.sample_rate_hz,
        normalized_channels: normalized.channels,
    };

    let transcript = engine.transcribe(&normalized.path, source, normalized.duration_ms)?;
    let transcript = output::TranscriptOutput::from_transcript(transcript, mode, copy);

    if copy {
        clipboard::copy_text(&transcript.final_text)?;
        eprintln!("Copied final text to clipboard.");
    }

    eprintln!(
        "Stop-to-final latency: {} ms.",
        stop_to_final.elapsed().as_millis()
    );
    output::print_transcript(&transcript, format)
}
