use std::{fs, path::Path};

use serde::Serialize;

use crate::{
    clipboard::{self, ClipboardCommandState},
    config::{self, ConfigFormat, ResolvedConfig},
    deps::{self, DependencyReport, DependencyState},
    error::ComlinkError,
    system_audio::{self, SystemAudioReport},
};

const DOCTOR_SCHEMA_VERSION: &str = "comlink.doctor.v1";

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub schema_version: String,
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
    pub paths: DoctorPaths,
    pub privacy: DoctorPrivacy,
    pub system_audio: SystemAudioReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub detail: String,
    pub remediation: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorPaths {
    pub home_dir: String,
    pub config_file: String,
    pub data_dir: String,
    pub database_file: String,
    pub audio_dir: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorPrivacy {
    pub history_enabled: bool,
    pub retain_metadata: bool,
    pub retain_transcripts: bool,
    pub retain_audio: bool,
    pub local_llm: String,
    pub posture: String,
}

pub fn run(format: ConfigFormat) -> Result<bool, ComlinkError> {
    let resolved = config::load(config::CliConfigOverrides::default())?;
    let selected_model = config::selected_model_path(&resolved.config);
    let dependencies = deps::inspect_with_model_path(selected_model);
    let report = build_report(&resolved, &dependencies);

    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        ConfigFormat::Text => print_report(&report),
    }

    Ok(report.ok)
}

fn build_report(resolved: &ResolvedConfig, dependencies: &DependencyReport) -> DoctorReport {
    let clipboard = clipboard::inspect();
    let system_audio = system_audio::inspect(dependencies);
    let mut checks = vec![
        dependency_check(
            "ffmpeg",
            &dependencies.ffmpeg,
            true,
            "FFmpeg normalizes files and records microphone audio.",
            "set COMLINK_FFMPEG or install ffmpeg",
        ),
        dependency_check(
            "ffprobe",
            &dependencies.ffprobe,
            false,
            "FFprobe improves duration and audio metadata detection.",
            "optional but recommended; set COMLINK_FFPROBE or install ffprobe",
        ),
        dependency_check(
            "asr",
            &dependencies.whisper_cpp,
            true,
            "Local ASR uses whisper.cpp through whisper-cli.",
            "set COMLINK_WHISPER_CPP to whisper-cli or install whisper.cpp",
        ),
        dependency_check(
            "model-path",
            &dependencies.whisper_model,
            true,
            "Selected whisper.cpp ggml model file.",
            "set COMLINK_WHISPER_MODEL or run comlink models select <name> --path <file>",
        ),
        clipboard_check(
            "clipboard-copy",
            &clipboard.copy,
            true,
            "Clipboard delivery uses pbcopy or COMLINK_PBCOPY.",
            "install macOS pbcopy support or set COMLINK_PBCOPY to an executable test adapter",
        ),
        clipboard_check(
            "clipboard-read",
            &clipboard.read,
            false,
            "Clipboard restore uses pbpaste or COMLINK_PBPASTE.",
            "optional; install macOS pbpaste support or set COMLINK_PBPASTE",
        ),
        data_path_check(&resolved.paths.data_dir),
    ];
    checks.push(system_audio_check(&system_audio));

    let record_device = std::env::var("COMLINK_RECORD_DEVICE").unwrap_or_else(|_| ":0".to_string());
    checks.push(DoctorCheck {
        name: "microphone".to_string(),
        status: "info".to_string(),
        required: false,
        path: None,
        detail: format!("record uses FFmpeg AVFoundation input device {record_device}"),
        remediation: "grant Terminal microphone permission in macOS Privacy & Security; override with COMLINK_RECORD_DEVICE or record --device".to_string(),
    });

    let ok = checks
        .iter()
        .all(|check| !check.required || check.status == "ok");

    DoctorReport {
        schema_version: DOCTOR_SCHEMA_VERSION.to_string(),
        ok,
        checks,
        paths: DoctorPaths {
            home_dir: resolved.paths.home_dir.display().to_string(),
            config_file: resolved.paths.config_file.display().to_string(),
            data_dir: resolved.paths.data_dir.display().to_string(),
            database_file: resolved.paths.database_file.display().to_string(),
            audio_dir: resolved.paths.audio_dir.display().to_string(),
        },
        privacy: DoctorPrivacy {
            history_enabled: resolved.config.history_enabled,
            retain_metadata: resolved.config.retention.metadata,
            retain_transcripts: resolved.config.retention.transcripts,
            retain_audio: resolved.config.retention.audio,
            local_llm: if resolved.config.llm.enabled {
                format!(
                    "enabled; provider={}; endpoint={}; model={}",
                    resolved.config.llm.provider.as_str(),
                    resolved.config.llm.endpoint,
                    resolved.config.llm.model.as_deref().unwrap_or("<none>")
                )
            } else {
                "disabled".to_string()
            },
            posture: "local-first; ASR runs through local whisper.cpp; LLM rewrite is opt-in"
                .to_string(),
        },
        system_audio,
    }
}

fn dependency_check(
    name: &str,
    state: &DependencyState,
    required: bool,
    detail: &str,
    remediation: &str,
) -> DoctorCheck {
    let (status, path, detail) = match state {
        DependencyState::Found(path) => (
            "ok".to_string(),
            Some(path.display().to_string()),
            detail.to_string(),
        ),
        DependencyState::Missing => (
            "missing".to_string(),
            None,
            format!("{detail} Not configured or not found on PATH."),
        ),
        DependencyState::NotFound(path) => (
            "missing".to_string(),
            Some(path.display().to_string()),
            format!("{detail} Configured path does not exist."),
        ),
        DependencyState::NotExecutable(path) => (
            "bad".to_string(),
            Some(path.display().to_string()),
            format!("{detail} Configured path is not executable."),
        ),
    };

    DoctorCheck {
        name: name.to_string(),
        status,
        required,
        path,
        detail,
        remediation: remediation.to_string(),
    }
}

fn clipboard_check(
    name: &str,
    state: &ClipboardCommandState,
    required: bool,
    detail: &str,
    remediation: &str,
) -> DoctorCheck {
    let (status, detail) = match state {
        ClipboardCommandState::Found(_) => ("ok".to_string(), detail.to_string()),
        ClipboardCommandState::Missing(_) => (
            "missing".to_string(),
            format!("{detail} Command was not found."),
        ),
        ClipboardCommandState::NotExecutable(_) => (
            "bad".to_string(),
            format!("{detail} Configured path is not executable."),
        ),
    };

    DoctorCheck {
        name: name.to_string(),
        status,
        required,
        path: Some(state.path().display().to_string()),
        detail,
        remediation: remediation.to_string(),
    }
}

fn data_path_check(path: &Path) -> DoctorCheck {
    let writable = if path.exists() {
        fs::metadata(path)
            .map(|metadata| metadata.is_dir() && !metadata.permissions().readonly())
            .unwrap_or(false)
    } else {
        nearest_existing_ancestor(path)
            .map(|ancestor| !is_readonly(ancestor))
            .unwrap_or(false)
    };

    DoctorCheck {
        name: "data-path".to_string(),
        status: if writable { "ok" } else { "bad" }.to_string(),
        required: true,
        path: Some(path.display().to_string()),
        detail: if writable {
            "Data directory is present or can be created.".to_string()
        } else {
            "Data directory cannot be created from the current parent path.".to_string()
        },
        remediation: "set COMLINK_DATA_DIR to a writable directory".to_string(),
    }
}

fn system_audio_check(report: &SystemAudioReport) -> DoctorCheck {
    DoctorCheck {
        name: "system-audio".to_string(),
        status: report.status.clone(),
        required: false,
        path: report.dependency.device_name.clone(),
        detail: report.detail.clone(),
        remediation: report.remediation.clone(),
    }
}

fn is_readonly(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.permissions().readonly())
        .unwrap_or(true)
}

fn nearest_existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|ancestor| ancestor.exists())
}

fn print_report(report: &DoctorReport) {
    eprintln!("Comlink doctor");
    eprintln!("  offline core: enabled");
    for check in &report.checks {
        let marker = match check.status.as_str() {
            "ok" => "[ok]  ",
            "info" => "[info]",
            "bad" => "[bad] ",
            _ => "[miss]",
        };
        if let Some(path) = &check.path {
            eprintln!(
                "  {marker} {}: {} ({})",
                check.name, path, check.remediation
            );
        } else {
            eprintln!("  {marker} {}: {}", check.name, check.remediation);
        }
    }
    eprintln!("  data_dir: {}", report.paths.data_dir);
    eprintln!(
        "  system_audio_permissions: microphone={}; routing={}",
        report.system_audio.permissions.microphone.status,
        report.system_audio.permissions.system_audio_routing.status
    );
    eprintln!("  privacy: {}", report.privacy.posture);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn required_missing_dependencies_make_report_unhealthy() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = ResolvedConfig {
            paths: config::ConfigPaths {
                home_dir: dir.path().to_path_buf(),
                config_file: dir.path().join("config.json"),
                data_dir: dir.path().join("data"),
                database_file: dir.path().join("data/history.sqlite3"),
                audio_dir: dir.path().join("data/audio"),
            },
            config: config::Config::default(),
            sources: vec!["defaults".to_string()],
        };
        let dependencies = DependencyReport {
            ffmpeg: DependencyState::Missing,
            ffprobe: DependencyState::Missing,
            whisper_cpp: DependencyState::Missing,
            whisper_model: DependencyState::NotFound(PathBuf::from("/missing/model.bin")),
        };

        let report = build_report(&resolved, &dependencies);

        assert!(!report.ok);
        assert!(report.checks.iter().any(|check| {
            check.name == "model-path" && check.status == "missing" && check.required
        }));
        assert!(report.checks.iter().any(|check| check.name == "microphone"));
        assert!(report
            .checks
            .iter()
            .any(|check| check.name == "system-audio" && !check.required));
        assert_eq!(
            report.system_audio.strategy,
            "blackhole-virtual-audio-device"
        );
        assert!(report
            .system_audio
            .source_metadata
            .labels
            .contains(&"user_mic".to_string()));
    }
}
