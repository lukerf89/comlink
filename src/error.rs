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
        "no speech transcribed: captured audio from device {device} was near-silent ({mean_dbfs:.1} dBFS avg) - likely wrong input device, muted mic, or missing microphone permission; set COMLINK_RECORD_DEVICE or run `comlink doctor --probe-mic`"
    )]
    NoSpeechNearSilent { mean_dbfs: f64, device: String },

    #[error(
        "recording too short or no speech detected: captured {duration_ms} ms, minimum is {min_duration_ms} ms"
    )]
    RecordingTooShort {
        duration_ms: u64,
        min_duration_ms: u64,
    },

    #[error("clipboard delivery failed: {0}")]
    ClipboardFailed(String),

    #[error("config home environment variable is required: {0}")]
    ConfigHomeMissing(&'static str),

    #[error("invalid config value {name}={value}")]
    InvalidConfigValue { name: &'static str, value: String },

    #[error("failed to parse config file {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        source: serde_json::Error,
    },

    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),

    #[error("history session not found: {0}")]
    HistoryNotFound(String),

    #[error("meeting session not found: {0}")]
    MeetingSessionNotFound(String),

    #[error("no active meeting session")]
    MeetingNoActiveSession,

    #[error("meeting session is already recording: {0}")]
    MeetingAlreadyActive(String),

    #[error("meeting session is not recording: {0}")]
    MeetingNotRecording(String),

    #[error("meeting session has not stopped yet: {0}")]
    MeetingNotStopped(String),

    #[error("meeting export is not available: {0}")]
    MeetingExportUnavailable(PathBuf),

    #[error("meeting session is not awaiting finalize (still recording): {0}")]
    MeetingNotTranscribing(String),

    #[error("meeting session is busy; another comlink process holds its lifecycle lock: {0}")]
    MeetingLifecycleBusy(String),

    #[error("meeting finalizer could not be launched: {0}")]
    MeetingFinalizeLaunchFailed(String),

    #[error("{kind} not found: {name}")]
    NotFound { kind: &'static str, name: String },

    #[error("text mode not found: {0}")]
    ModeNotFound(String),

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
            Self::EmptyTranscript
            | Self::NoSpeechNearSilent { .. }
            | Self::RecordingTooShort { .. } => 4,
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

    #[test]
    fn near_silent_no_speech_keeps_exit_code_four_and_names_remediation() {
        let error = ComlinkError::NoSpeechNearSilent {
            mean_dbfs: -120.0,
            device: ":0".to_string(),
        };

        assert_eq!(error.exit_code(), 4);
        let message = error.to_string();
        assert!(message.contains("near-silent"));
        assert!(message.contains("-120.0 dBFS"));
        assert!(message.contains("COMLINK_RECORD_DEVICE"));
        assert!(message.contains("doctor --probe-mic"));
    }

    #[test]
    fn phase_four_exit_codes_cover_agent_contract_cases() {
        assert_eq!(
            ComlinkError::InputMissing("missing.wav".into()).exit_code(),
            1
        );
        assert_eq!(ComlinkError::ModelMissing.exit_code(), 3);
        assert_eq!(ComlinkError::EmptyTranscript.exit_code(), 4);
        assert_eq!(
            ComlinkError::ClipboardFailed("pbcopy failed".to_string()).exit_code(),
            5
        );
    }

    #[test]
    fn meeting_lifecycle_errors_use_general_exit_code() {
        assert_eq!(
            ComlinkError::MeetingNotTranscribing("s1".to_string()).exit_code(),
            1
        );
        assert_eq!(
            ComlinkError::MeetingLifecycleBusy("s1".to_string()).exit_code(),
            1
        );
        assert_eq!(
            ComlinkError::MeetingFinalizeLaunchFailed("spawn failed".to_string()).exit_code(),
            1
        );
        // Existing codes are unchanged, so a finalize that surfaces a whisper
        // failure still exits 3.
        assert_eq!(
            ComlinkError::WhisperFailed("mock".to_string()).exit_code(),
            3
        );
    }
}
