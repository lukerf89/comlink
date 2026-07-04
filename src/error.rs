use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ComlinkError {
    #[error("input file does not exist: {0}")]
    InputMissing(PathBuf),

    #[error("dependency not found: {0}")]
    DependencyMissing(&'static str),

    #[error("dependency path does not exist: {name}={path}")]
    DependencyPathMissing { name: &'static str, path: PathBuf },

    #[error("dependency path is not executable: {name}={path}")]
    DependencyNotExecutable { name: &'static str, path: PathBuf },

    #[error("model path is required; set COMLINK_WHISPER_MODEL or pass --model")]
    ModelMissing,

    #[error("model path does not exist: {0}")]
    ModelPathMissing(PathBuf),

    #[error("ffmpeg failed to normalize audio: {0}")]
    FfmpegFailed(String),

    #[error("audio capture failed: {0}")]
    AudioCaptureFailed(String),

    #[error("whisper.cpp failed: {0}")]
    WhisperFailed(String),

    #[error("whisper.cpp produced no transcript text")]
    EmptyTranscript,

    #[error(
        "recording too short or no speech detected: captured {duration_ms} ms, minimum is {min_duration_ms} ms"
    )]
    RecordingTooShort {
        duration_ms: u64,
        min_duration_ms: u64,
    },

    #[error("clipboard delivery failed: {0}")]
    ClipboardFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl ComlinkError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::ModelMissing | Self::ModelPathMissing(_) | Self::WhisperFailed(_) => 3,
            Self::AudioCaptureFailed(_) => 2,
            Self::EmptyTranscript | Self::RecordingTooShort { .. } => 4,
            Self::ClipboardFailed(_) => 5,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_too_short_uses_no_speech_exit_code() {
        let error = ComlinkError::RecordingTooShort {
            duration_ms: 0,
            min_duration_ms: 300,
        };

        assert_eq!(error.exit_code(), 4);
        assert!(error.to_string().contains("recording too short"));
    }
}
