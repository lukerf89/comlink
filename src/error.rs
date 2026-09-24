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

    #[error("meeting export at {path} is invalid ({reason}); fix or remove the invalid export at {path}, then rerun `comlink meet finalize {id}`")]
    MeetingExportInvalid {
        id: String,
        path: PathBuf,
        reason: String,
    },

    #[error("meeting session is not awaiting finalize (still recording): {0}")]
    MeetingNotTranscribing(String),

    #[error("meeting session is busy; another comlink process holds its lifecycle lock: {0}")]
    MeetingLifecycleBusy(String),

    #[error("meeting finalizer could not be launched: {0}")]
    MeetingFinalizeLaunchFailed(String),

    #[error("meeting session is still transcribing: {0}; run `comlink meet status {0}`")]
    MeetingStillTranscribing(String),

    #[error("meeting session finalize failed: {0}; run `comlink meet status {0}`, then `comlink meet finalize {0}` to retry")]
    MeetingFinalizeFailed(String),

    /// `MeetingFinalizeFailed` with the recorded error text, for callers (the
    /// MCP server) that must show the agent why finalize failed.
    #[error("meeting session finalize failed: {id}: {error}; run `comlink meet status {id}`, then `comlink meet finalize {id}` to retry")]
    MeetingFinalizeFailedDetail { id: String, error: String },

    #[error("meeting_start is disabled for MCP clients (mcp.allow_start=false); enable it with `comlink config set mcp.allow_start true`")]
    McpStartDisabled,

    #[error("unknown config key: {0}; supported keys: mcp.allow_start")]
    UnknownConfigKey(String),

    #[error("meeting audio chunk cleanup failed for {id} (retention.audio=false): {reason}; rerun `comlink meet finalize {id}`")]
    MeetingChunkCleanupFailed { id: String, reason: String },

    #[error("meeting session {id} is unreadable: {reason}; inspect or remove {path}")]
    MeetingSessionUnreadable {
        id: String,
        path: PathBuf,
        reason: String,
    },

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

    /// Stable, machine-readable identifier for this error, part of the public
    /// contract (MCP `structuredContent.error_code`). The match is exhaustive
    /// on purpose: a new variant does not compile until it is given a code.
    /// `MeetingFinalizeFailed` and `MeetingFinalizeFailedDetail` describe the
    /// same condition and share a code.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InputMissing(_) => "input_missing",
            Self::DependencyMissing(_) => "dependency_missing",
            Self::DependencyPathMissing { .. } => "dependency_path_missing",
            Self::DependencyNotExecutable { .. } => "dependency_not_executable",
            Self::ModelMissing => "model_missing",
            Self::ModelPathMissing(_) => "model_path_missing",
            Self::FfmpegFailed(_) => "ffmpeg_failed",
            Self::AudioCaptureFailed(_) => "audio_capture_failed",
            Self::WhisperFailed(_) => "whisper_failed",
            Self::EmptyTranscript => "empty_transcript",
            Self::NoSpeechNearSilent { .. } => "no_speech_near_silent",
            Self::RecordingTooShort { .. } => "recording_too_short",
            Self::ClipboardFailed(_) => "clipboard_failed",
            Self::ConfigHomeMissing(_) => "config_home_missing",
            Self::InvalidConfigValue { .. } => "invalid_config_value",
            Self::ConfigParse { .. } => "config_parse",
            Self::Storage(_) => "storage",
            Self::HistoryNotFound(_) => "history_not_found",
            Self::MeetingSessionNotFound(_) => "meeting_session_not_found",
            Self::MeetingNoActiveSession => "meeting_no_active_session",
            Self::MeetingAlreadyActive(_) => "meeting_already_active",
            Self::MeetingNotRecording(_) => "meeting_not_recording",
            Self::MeetingNotStopped(_) => "meeting_not_stopped",
            Self::MeetingExportUnavailable(_) => "meeting_export_unavailable",
            Self::MeetingExportInvalid { .. } => "meeting_export_invalid",
            Self::MeetingNotTranscribing(_) => "meeting_not_transcribing",
            Self::MeetingLifecycleBusy(_) => "meeting_lifecycle_busy",
            Self::MeetingFinalizeLaunchFailed(_) => "meeting_finalize_launch_failed",
            Self::MeetingStillTranscribing(_) => "meeting_still_transcribing",
            Self::MeetingFinalizeFailed(_) | Self::MeetingFinalizeFailedDetail { .. } => {
                "meeting_finalize_failed"
            }
            Self::McpStartDisabled => "mcp_start_disabled",
            Self::UnknownConfigKey(_) => "unknown_config_key",
            Self::MeetingChunkCleanupFailed { .. } => "meeting_chunk_cleanup_failed",
            Self::MeetingSessionUnreadable { .. } => "meeting_session_unreadable",
            Self::NotFound { .. } => "not_found",
            Self::ModeNotFound(_) => "mode_not_found",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One instance of every variant. Adding a variant fails `error_code`'s
    /// exhaustive match; add it here too so its code is checked.
    fn every_variant() -> Vec<ComlinkError> {
        let json_error = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        vec![
            ComlinkError::InputMissing("a".into()),
            ComlinkError::DependencyMissing("ffmpeg"),
            ComlinkError::DependencyPathMissing {
                name: "ffmpeg",
                path: "a".into(),
            },
            ComlinkError::DependencyNotExecutable {
                name: "ffmpeg",
                path: "a".into(),
            },
            ComlinkError::ModelMissing,
            ComlinkError::ModelPathMissing("a".into()),
            ComlinkError::FfmpegFailed("a".into()),
            ComlinkError::AudioCaptureFailed("a".into()),
            ComlinkError::WhisperFailed("a".into()),
            ComlinkError::EmptyTranscript,
            ComlinkError::NoSpeechNearSilent {
                mean_dbfs: -90.0,
                device: ":0".into(),
            },
            ComlinkError::RecordingTooShort {
                duration_ms: 0,
                min_duration_ms: 1,
            },
            ComlinkError::ClipboardFailed("a".into()),
            ComlinkError::ConfigHomeMissing("HOME"),
            ComlinkError::InvalidConfigValue {
                name: "x",
                value: "y".into(),
            },
            ComlinkError::ConfigParse {
                path: "a".into(),
                source: serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
            },
            ComlinkError::Storage(rusqlite::Error::InvalidQuery),
            ComlinkError::HistoryNotFound("a".into()),
            ComlinkError::MeetingSessionNotFound("a".into()),
            ComlinkError::MeetingNoActiveSession,
            ComlinkError::MeetingAlreadyActive("a".into()),
            ComlinkError::MeetingNotRecording("a".into()),
            ComlinkError::MeetingNotStopped("a".into()),
            ComlinkError::MeetingExportUnavailable("a".into()),
            ComlinkError::MeetingExportInvalid {
                id: "a".into(),
                path: "a".into(),
                reason: "r".into(),
            },
            ComlinkError::MeetingNotTranscribing("a".into()),
            ComlinkError::MeetingLifecycleBusy("a".into()),
            ComlinkError::MeetingFinalizeLaunchFailed("a".into()),
            ComlinkError::MeetingStillTranscribing("a".into()),
            ComlinkError::MeetingFinalizeFailed("a".into()),
            ComlinkError::MeetingFinalizeFailedDetail {
                id: "a".into(),
                error: "e".into(),
            },
            ComlinkError::McpStartDisabled,
            ComlinkError::UnknownConfigKey("a".into()),
            ComlinkError::MeetingChunkCleanupFailed {
                id: "a".into(),
                reason: "r".into(),
            },
            ComlinkError::MeetingSessionUnreadable {
                id: "a".into(),
                path: "a".into(),
                reason: "r".into(),
            },
            ComlinkError::NotFound {
                kind: "mode",
                name: "a".into(),
            },
            ComlinkError::ModeNotFound("a".into()),
            ComlinkError::Io(std::io::Error::other("a")),
            ComlinkError::Json(json_error),
        ]
    }

    #[test]
    fn error_codes_are_unique_snake_case_and_stable() {
        let mut seen = std::collections::BTreeMap::new();
        for error in every_variant() {
            let code = error.error_code();
            assert!(!code.is_empty());
            assert!(
                code.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{code} is not snake_case"
            );
            if let Some(previous) = seen.insert(code, format!("{error:?}")) {
                // The only intended alias: the detailed finalize failure.
                assert_eq!(
                    code, "meeting_finalize_failed",
                    "{code} reused by {previous}"
                );
            }
        }
        // One documented alias; every other variant has its own code.
        assert_eq!(seen.len(), every_variant().len() - 1);
        // Codes the MCP contract documents.
        for (error, code) in [
            (ComlinkError::McpStartDisabled, "mcp_start_disabled"),
            (
                ComlinkError::MeetingNoActiveSession,
                "meeting_no_active_session",
            ),
            (
                ComlinkError::MeetingAlreadyActive("a".into()),
                "meeting_already_active",
            ),
            (
                ComlinkError::MeetingStillTranscribing("a".into()),
                "meeting_still_transcribing",
            ),
            (
                ComlinkError::MeetingSessionNotFound("a".into()),
                "meeting_session_not_found",
            ),
            (ComlinkError::ModeNotFound("a".into()), "mode_not_found"),
        ] {
            assert_eq!(error.error_code(), code);
        }
    }

    #[test]
    fn mcp_and_config_errors_name_their_remedy_and_use_general_exit_code() {
        let disabled = ComlinkError::McpStartDisabled;
        assert_eq!(disabled.exit_code(), 1);
        assert!(disabled
            .to_string()
            .contains("comlink config set mcp.allow_start true"));

        let detail = ComlinkError::MeetingFinalizeFailedDetail {
            id: "s1".into(),
            error: "whisper.cpp failed: boom".into(),
        };
        assert_eq!(
            detail.exit_code(),
            ComlinkError::MeetingFinalizeFailed("s1".into()).exit_code()
        );
        let message = detail.to_string();
        assert!(message.contains("whisper.cpp failed: boom"));
        assert!(message.contains("comlink meet finalize s1"));

        let unknown = ComlinkError::UnknownConfigKey("mcp.nope".into());
        assert_eq!(unknown.exit_code(), 1);
        assert!(unknown.to_string().contains("mcp.allow_start"));
    }

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
        assert_eq!(
            ComlinkError::MeetingStillTranscribing("s1".to_string()).exit_code(),
            1
        );
        assert_eq!(
            ComlinkError::MeetingFinalizeFailed("s1".to_string()).exit_code(),
            1
        );
        assert_eq!(
            ComlinkError::MeetingChunkCleanupFailed {
                id: "s1".to_string(),
                reason: "denied".to_string(),
            }
            .exit_code(),
            1
        );
        assert_eq!(
            ComlinkError::MeetingExportInvalid {
                id: "s1".to_string(),
                path: "s1/transcript.json".into(),
                reason: "eof".to_string(),
            }
            .exit_code(),
            1
        );
        assert_eq!(
            ComlinkError::MeetingSessionUnreadable {
                id: "s1".to_string(),
                path: "s1/session.json".into(),
                reason: "eof".to_string(),
            }
            .exit_code(),
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
