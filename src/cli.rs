use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{
    asr::{AsrEngine, SourceMetadata, WhisperCppEngine},
    audio, deps, doctor,
    error::ComlinkError,
    output::{self, OutputFormat},
};

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

        /// whisper.cpp ggml model path. Defaults to COMLINK_WHISPER_MODEL.
        #[arg(long)]
        model: Option<PathBuf>,
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
            model,
        } => transcribe(file, format, model),
    }
}

fn transcribe(
    file: PathBuf,
    format: OutputFormat,
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
    output::print_transcript(&transcript, format)
}
