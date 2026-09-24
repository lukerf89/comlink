use std::{fs, path::Path};

use serde::Serialize;

use crate::{
    audio,
    clipboard::{self, ClipboardCommandState},
    config::{self, ConfigFormat, ResolvedConfig},
    deps::{self, DependencyReport, DependencyState},
    error::ComlinkError,
    record::{self, MicProbe, ProbeOutcome, ResolvedRecordDevice},
    system_audio::{self, SystemAudioReport},
};

const DOCTOR_SCHEMA_VERSION: &str = "comlink.doctor.v1";

/// Files strictly smaller than this are treated as a test stub rather than a
/// real whisper.cpp ggml model. The whisper.cpp `for-tests-*` stubs are about
/// 562 KB, while the smallest real ggml model (tiny, q5_1) is about 31 MB, so a
/// 10 MB threshold separates them with wide margin on both sides. Exactly
/// 10,000,000 bytes is treated as a real model.
pub const MODEL_STUB_MAX_BYTES: u64 = 10_000_000;

/// Doctor switches from the CLI.
#[derive(Debug, Clone, Copy, Default)]
pub struct DoctorOptions {
    /// Run a short live microphone capture and report whether it has signal.
    pub probe_mic: bool,
}

/// Pure classification of a model file by size and file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelAssessment {
    Ok,
    LooksLikeStub { reason: String },
}

/// Classify a model file. `basename` must be the file name only so a
/// `for-tests-` segment in a parent directory never flags a real model.
pub fn assess_model_file(size: u64, basename: &str) -> ModelAssessment {
    if basename.contains("for-tests-") {
        return ModelAssessment::LooksLikeStub {
            reason: format!("file name {basename} is a whisper.cpp test model ({size} bytes)"),
        };
    }
    if size < MODEL_STUB_MAX_BYTES {
        return ModelAssessment::LooksLikeStub {
            reason: format!(
                "{size} bytes is below the {MODEL_STUB_MAX_BYTES} byte minimum for a real ggml model"
            ),
        };
    }
    ModelAssessment::Ok
}

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

pub fn run(format: ConfigFormat, options: DoctorOptions) -> Result<bool, ComlinkError> {
    let resolved = config::load(config::CliConfigOverrides::default())?;
    let selected_model = config::selected_model_path(&resolved.config);
    let dependencies = deps::inspect_with_model_path(selected_model);
    let real_probe = match &dependencies.ffmpeg {
        DependencyState::Found(path) if options.probe_mic => {
            Some(record::FfmpegMicProbe::new(path.clone()))
        }
        _ => None,
    };
    let report = build_report_with(
        &resolved,
        &dependencies,
        options,
        real_probe.as_ref().map(|probe| probe as &dyn MicProbe),
    );

    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        ConfigFormat::Text => print_report(&report),
    }

    Ok(report.ok)
}

#[cfg(test)]
fn build_report(resolved: &ResolvedConfig, dependencies: &DependencyReport) -> DoctorReport {
    build_report_with(resolved, dependencies, DoctorOptions::default(), None)
}

/// Build the doctor report. `probe` is only consulted when
/// `options.probe_mic` is set; `None` there means ffmpeg is unavailable.
fn build_report_with(
    resolved: &ResolvedConfig,
    dependencies: &DependencyReport,
    options: DoctorOptions,
    probe: Option<&dyn MicProbe>,
) -> DoctorReport {
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
        model_path_check(&dependencies.whisper_model),
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

    let ffmpeg = match &dependencies.ffmpeg {
        DependencyState::Found(path) => Some(path.as_path()),
        _ => None,
    };
    checks.push(microphone_check(ffmpeg, options, probe));

    let ok = report_ok(&checks);

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

/// Overall health: every required check is `ok` or `warn`. `warn` means
/// degraded-but-usable, so it never flips `ok` or the exit code.
fn report_ok(checks: &[DoctorCheck]) -> bool {
    checks
        .iter()
        .all(|check| !check.required || matches!(check.status.as_str(), "ok" | "warn"))
}

/// `model-path` check: the dependency check plus a stub-model heuristic. A
/// stub is `warn` (degraded but runnable), which keeps `ok` and the exit code.
fn model_path_check(state: &DependencyState) -> DoctorCheck {
    let mut check = dependency_check(
        "model-path",
        state,
        true,
        "Selected whisper.cpp ggml model file.",
        "set COMLINK_WHISPER_MODEL or run comlink models select <name> --path <file>",
    );
    let DependencyState::Found(path) = state else {
        return check;
    };
    let basename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match fs::metadata(path) {
        Ok(metadata) => {
            if let ModelAssessment::LooksLikeStub { reason } =
                assess_model_file(metadata.len(), &basename)
            {
                check.status = "warn".to_string();
                check.detail = format!(
                    "Selected model looks like a test stub / not a real model ({} bytes): {reason}. Transcription will produce garbage or nothing.",
                    metadata.len()
                );
                check.remediation =
                    "run `comlink models select <name> --path <real ggml model>`".to_string();
            }
        }
        Err(error) => {
            check.status = "warn".to_string();
            check.detail =
                format!("Selected model exists but could not read model file size: {error}");
        }
    }
    check
}

const MICROPHONE_REMEDIATION: &str = "grant Terminal microphone permission in macOS Privacy & Security; override with COMLINK_RECORD_DEVICE or record --device";

fn microphone_check(
    ffmpeg: Option<&Path>,
    options: DoctorOptions,
    probe: Option<&dyn MicProbe>,
) -> DoctorCheck {
    let resolved = match ffmpeg {
        Some(ffmpeg) => record::resolve_record_device(None, ffmpeg),
        None => Ok(ResolvedRecordDevice {
            avfoundation_input: record::DEFAULT_RECORD_DEVICE.to_string(),
            name: None,
            source: record::DeviceSource::Fallback,
        }),
    };
    let device = match resolved {
        Ok(device) => device,
        Err(error) => {
            // Resolution failures are diagnostics, never a doctor failure.
            return DoctorCheck {
                name: "microphone".to_string(),
                status: "warn".to_string(),
                required: false,
                path: None,
                detail: format!("record input device could not be resolved: {error}"),
                remediation: MICROPHONE_REMEDIATION.to_string(),
            };
        }
    };

    if options.probe_mic {
        let outcome = match probe {
            Some(probe) => probe.probe(&device.avfoundation_input),
            None => ProbeOutcome::CaptureFailed {
                stderr_tail: "ffmpeg is not available to run the probe".to_string(),
            },
        };
        return microphone_probe_check(&device, outcome);
    }

    DoctorCheck {
        name: "microphone".to_string(),
        status: "info".to_string(),
        required: false,
        path: None,
        detail: microphone_device_detail(&device),
        remediation: format!(
            "{MICROPHONE_REMEDIATION}; run `comlink doctor --probe-mic` to test for signal"
        ),
    }
}

fn microphone_device_detail(device: &ResolvedRecordDevice) -> String {
    match device.source {
        record::DeviceSource::Flag | record::DeviceSource::Env => format!(
            "record uses FFmpeg AVFoundation input device {} (COMLINK_RECORD_DEVICE)",
            device.label()
        ),
        record::DeviceSource::SystemDefault => format!(
            "record defaults to the system default input device {}",
            device.label()
        ),
        record::DeviceSource::Fallback => format!(
            "record defaults to AVFoundation input device {} (system default input could not be resolved)",
            device.label()
        ),
    }
}

/// Pure mapping of a live probe outcome onto the (never required) microphone
/// check. Nothing here fails the doctor: problems are `warn`.
pub fn microphone_probe_check(device: &ResolvedRecordDevice, outcome: ProbeOutcome) -> DoctorCheck {
    let base = microphone_device_detail(device);
    let (status, detail, remediation) = match outcome {
        ProbeOutcome::Level(level) if level.is_near_silent() => (
            "warn",
            format!(
                "no signal from device {} (mean {:.1} dBFS, peak {:.1} dBFS); {base}",
                device.label(),
                level.mean_dbfs,
                level.peak_dbfs
            ),
            audio::near_silent_device_hint(audio::DeviceHintContext {
                selector: &device.avfoundation_input,
                name: device.name.as_deref(),
                available: None,
            }),
        ),
        ProbeOutcome::Level(level) => (
            "ok",
            format!(
                "live probe heard signal from device {} (mean {:.1} dBFS, peak {:.1} dBFS); {base}",
                device.label(),
                level.mean_dbfs,
                level.peak_dbfs
            ),
            MICROPHONE_REMEDIATION.to_string(),
        ),
        ProbeOutcome::CaptureFailed { stderr_tail } => (
            "warn",
            format!(
                "live probe capture failed on device {}: {stderr_tail}",
                device.label()
            ),
            MICROPHONE_REMEDIATION.to_string(),
        ),
        ProbeOutcome::TimedOut => (
            "warn",
            format!(
                "live probe on device {} timed out and was stopped; the device may be busy or waiting on a permission prompt",
                device.label()
            ),
            MICROPHONE_REMEDIATION.to_string(),
        ),
        ProbeOutcome::Unmeasurable => (
            "warn",
            format!(
                "live probe on device {} produced audio that could not be measured",
                device.label()
            ),
            MICROPHONE_REMEDIATION.to_string(),
        ),
    };

    DoctorCheck {
        name: "microphone".to_string(),
        status: status.to_string(),
        required: false,
        path: None,
        detail,
        remediation,
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
            "warn" => "[warn]",
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
        if check.status == "warn" {
            eprintln!("         {}", check.detail);
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

    #[test]
    fn model_assessment_uses_strict_size_threshold() {
        assert!(matches!(
            assess_model_file(0, "ggml-base.bin"),
            ModelAssessment::LooksLikeStub { .. }
        ));
        assert!(matches!(
            assess_model_file(9_999_999, "ggml-base.bin"),
            ModelAssessment::LooksLikeStub { .. }
        ));
        assert_eq!(
            assess_model_file(10_000_000, "ggml-base.bin"),
            ModelAssessment::Ok
        );
        assert_eq!(
            assess_model_file(31_000_000, "ggml-tiny-q5_1.bin"),
            ModelAssessment::Ok
        );
    }

    #[test]
    fn model_assessment_flags_for_tests_basename_regardless_of_size() {
        match assess_model_file(200_000_000, "ggml-for-tests-base.bin") {
            ModelAssessment::LooksLikeStub { reason } => assert!(reason.contains("test model")),
            other => panic!("expected stub, got {other:?}"),
        }
    }

    fn sparse_file(path: &Path, size: u64) {
        let file = fs::File::create(path).unwrap();
        file.set_len(size).unwrap();
    }

    #[test]
    fn model_path_check_warns_on_stub_but_stays_required() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("ggml-tiny.bin");
        fs::write(&model, b"mock model\n").unwrap();

        let check = model_path_check(&DependencyState::Found(model));

        assert_eq!(check.status, "warn");
        assert!(check.required);
        assert!(check
            .detail
            .contains("test stub / not a real model (11 bytes)"));
        assert!(check.remediation.contains("comlink models select"));
    }

    #[test]
    fn model_path_check_only_inspects_basename_for_test_marker() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("for-tests-models");
        fs::create_dir_all(&parent).unwrap();
        let model = parent.join("ggml-base.bin");
        sparse_file(&model, 16 << 20);

        assert_eq!(
            model_path_check(&DependencyState::Found(model)).status,
            "ok"
        );

        let stub = dir.path().join("ggml-for-tests-x.bin");
        sparse_file(&stub, 16 << 20);
        assert_eq!(
            model_path_check(&DependencyState::Found(stub)).status,
            "warn"
        );
    }

    #[test]
    fn model_path_check_warns_when_metadata_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let vanished = dir.path().join("vanished.bin");

        let check = model_path_check(&DependencyState::Found(vanished));

        assert_eq!(check.status, "warn");
        assert!(check.detail.contains("could not read model file size"));
    }

    fn check(name: &str, status: &str, required: bool) -> DoctorCheck {
        DoctorCheck {
            name: name.to_string(),
            status: status.to_string(),
            required,
            path: None,
            detail: String::new(),
            remediation: String::new(),
        }
    }

    #[test]
    fn required_warn_keeps_report_ok_but_missing_does_not() {
        assert!(report_ok(&[
            check("ffmpeg", "ok", true),
            check("model-path", "warn", true),
            check("microphone", "warn", false),
        ]));
        assert!(!report_ok(&[
            check("ffmpeg", "ok", true),
            check("model-path", "missing", true),
        ]));
        assert!(!report_ok(&[check("ffmpeg", "bad", true)]));
    }

    struct FakeProbe {
        outcome: ProbeOutcome,
        calls: std::cell::RefCell<Vec<String>>,
    }

    impl FakeProbe {
        fn new(outcome: ProbeOutcome) -> Self {
            Self {
                outcome,
                calls: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl MicProbe for FakeProbe {
        fn probe(&self, device: &str) -> ProbeOutcome {
            self.calls.borrow_mut().push(device.to_string());
            self.outcome.clone()
        }
    }

    fn level(mean_dbfs: f64) -> crate::audio::AudioLevel {
        crate::audio::AudioLevel {
            mean_dbfs,
            peak_dbfs: mean_dbfs + 10.0,
        }
    }

    #[test]
    fn microphone_check_never_probes_without_flag() {
        let fake = FakeProbe::new(ProbeOutcome::TimedOut);

        let result = microphone_check(None, DoctorOptions { probe_mic: false }, Some(&fake));

        assert!(fake.calls.borrow().is_empty());
        assert_eq!(result.status, "info");
        assert!(!result.required);
        assert!(result.remediation.contains("--probe-mic"));
    }

    #[test]
    fn microphone_check_probes_the_resolved_device_when_flagged() {
        let fake = FakeProbe::new(ProbeOutcome::Level(level(-120.0)));

        let result = microphone_check(None, DoctorOptions { probe_mic: true }, Some(&fake));

        assert_eq!(fake.calls.borrow().as_slice(), [":0".to_string()]);
        assert_eq!(result.status, "warn");
        assert!(result.detail.contains("no signal from device :0"));
    }

    #[test]
    fn microphone_probe_outcomes_map_to_ok_or_warn_never_fail() {
        let device = ResolvedRecordDevice {
            avfoundation_input: ":1".to_string(),
            name: Some("MacBook Pro Microphone".to_string()),
            source: record::DeviceSource::SystemDefault,
        };
        let cases = [
            (ProbeOutcome::Level(level(-25.0)), "ok", "heard signal"),
            (
                ProbeOutcome::Level(level(-90.0)),
                "warn",
                "no signal from device MacBook Pro Microphone (:1) (mean -90.0 dBFS",
            ),
            (
                ProbeOutcome::CaptureFailed {
                    stderr_tail: "Input/output error".to_string(),
                },
                "warn",
                "Input/output error",
            ),
            (ProbeOutcome::TimedOut, "warn", "timed out"),
            (ProbeOutcome::Unmeasurable, "warn", "could not be measured"),
        ];
        for (outcome, status, needle) in cases {
            let result = microphone_probe_check(&device, outcome);
            assert_eq!(result.status, status);
            assert!(!result.required);
            assert!(result.detail.contains(needle), "{}", result.detail);
        }
        let silent = microphone_probe_check(&device, ProbeOutcome::Level(level(-90.0)));
        assert!(silent.remediation.contains("COMLINK_RECORD_DEVICE"));
    }
}
