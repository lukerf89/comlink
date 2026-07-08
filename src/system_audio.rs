use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Serialize;

use crate::deps::{DependencyReport, DependencyState};

const BLACKHOLE_DEVICE_HINT: &str = "BlackHole";
const SYSTEM_AUDIO_DEVICE_ENV: &str = "COMLINK_SYSTEM_AUDIO_DEVICE";
const SYSTEM_AUDIO_FAKE_ENV: &str = "COMLINK_SYSTEM_AUDIO_FAKE";
const MIN_NATIVE_TAP_MACOS: MacOsVersion = MacOsVersion {
    major: 14,
    minor: 4,
    patch: 0,
};

#[derive(Debug, Clone, Serialize)]
pub struct SystemAudioReport {
    pub available: bool,
    pub status: String,
    pub strategy: String,
    pub detail: String,
    pub remediation: String,
    pub macos_version: Option<String>,
    pub dependency: SystemAudioDependency,
    pub permissions: SystemAudioPermissions,
    pub native_core_audio_tap: NativeCoreAudioTap,
    pub source_metadata: SourceMetadataPrototype,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemAudioDependency {
    pub name: String,
    pub present: bool,
    pub device_name: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemAudioPermissions {
    pub microphone: PermissionDiagnostic,
    pub system_audio_routing: PermissionDiagnostic,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionDiagnostic {
    pub status: String,
    pub detail: String,
    pub remediation: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeCoreAudioTap {
    pub supported_by_os: bool,
    pub minimum_macos: String,
    pub permission_model: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceMetadataPrototype {
    pub labels: Vec<String>,
    pub modes: Vec<String>,
    pub mixed_source_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemAudioProbeSnapshot {
    pub platform: Platform,
    pub audio_input_devices: Vec<String>,
    pub probe_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    MacOs(Option<MacOsVersion>),
    Other(String),
}

pub trait SystemAudioProbe {
    fn snapshot(&self) -> SystemAudioProbeSnapshot;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemAudioCapturePlan {
    pub device_name: String,
    pub avfoundation_input: String,
}

#[derive(Debug, Clone)]
pub struct RealSystemAudioProbe {
    ffmpeg: Option<PathBuf>,
}

impl RealSystemAudioProbe {
    pub fn new(dependencies: &DependencyReport) -> Self {
        Self {
            ffmpeg: match &dependencies.ffmpeg {
                DependencyState::Found(path) => Some(path.clone()),
                _ => None,
            },
        }
    }

    pub fn from_ffmpeg(ffmpeg: PathBuf) -> Self {
        Self {
            ffmpeg: Some(ffmpeg),
        }
    }
}

impl SystemAudioProbe for RealSystemAudioProbe {
    fn snapshot(&self) -> SystemAudioProbeSnapshot {
        if let Some(fake) = env::var_os(SYSTEM_AUDIO_FAKE_ENV) {
            return fake_snapshot(&fake.to_string_lossy());
        }

        let platform = current_platform();
        let mut probe_error = None;
        let audio_input_devices = if matches!(platform, Platform::MacOs(_)) {
            match self.ffmpeg.as_deref() {
                Some(ffmpeg) => match list_avfoundation_audio_devices(ffmpeg) {
                    Ok(devices) => devices,
                    Err(error) => {
                        probe_error = Some(error);
                        Vec::new()
                    }
                },
                None => {
                    probe_error = Some(
                        "ffmpeg is unavailable, so AVFoundation audio devices were not listed"
                            .to_string(),
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        SystemAudioProbeSnapshot {
            platform,
            audio_input_devices,
            probe_error,
        }
    }
}

pub fn inspect(dependencies: &DependencyReport) -> SystemAudioReport {
    inspect_with_probe(&RealSystemAudioProbe::new(dependencies))
}

pub fn inspect_with_probe(probe: &impl SystemAudioProbe) -> SystemAudioReport {
    report_from_snapshot(probe.snapshot(), preferred_device_name())
}

pub fn resolve_capture_plan(
    probe: &impl SystemAudioProbe,
) -> Result<SystemAudioCapturePlan, String> {
    let report = inspect_with_probe(probe);
    let Some(device_name) = report.dependency.device_name else {
        return Err(format!(
            "{} {}",
            report.detail.trim_end_matches('.'),
            report.remediation
        ));
    };
    Ok(SystemAudioCapturePlan {
        avfoundation_input: avfoundation_audio_input(&device_name),
        device_name,
    })
}

pub fn avfoundation_audio_input(device_name: &str) -> String {
    if device_name.starts_with(':') {
        device_name.to_string()
    } else {
        format!(":{device_name}")
    }
}

fn report_from_snapshot(
    snapshot: SystemAudioProbeSnapshot,
    preferred_device: Option<String>,
) -> SystemAudioReport {
    let macos_version = match &snapshot.platform {
        Platform::MacOs(version) => version.as_ref().map(ToString::to_string),
        Platform::Other(_) => None,
    };
    let native_supported = match &snapshot.platform {
        Platform::MacOs(Some(version)) => version >= &MIN_NATIVE_TAP_MACOS,
        _ => false,
    };
    let matching_device = find_loopback_device(&snapshot.audio_input_devices, preferred_device);
    let dependency_present = matching_device.is_some();

    let (available, status, detail, remediation) = match &snapshot.platform {
        Platform::Other(name) => (
            false,
            "unsupported-os".to_string(),
            format!("System-audio capture is scoped to macOS; current platform is {name}."),
            "Use Comlink system-audio capture diagnostics on macOS.".to_string(),
        ),
        Platform::MacOs(_) if dependency_present => (
            true,
            "ok".to_string(),
            "BlackHole virtual audio input is visible to AVFoundation.".to_string(),
            "Route meeting output through a Multi-Output or Aggregate Device that includes BlackHole, then capture the BlackHole input alongside the microphone in a future phase.".to_string(),
        ),
        Platform::MacOs(_) if snapshot.probe_error.is_some() => (
            false,
            "probe-error".to_string(),
            snapshot.probe_error.clone().unwrap_or_default(),
            "Install/configure ffmpeg, then rerun `comlink doctor --format json`; if BlackHole is installed, confirm it appears as an AVFoundation audio input.".to_string(),
        ),
        Platform::MacOs(_) => (
            false,
            "missing-dependency".to_string(),
            "No BlackHole virtual audio input was detected in AVFoundation devices.".to_string(),
            "Install BlackHole 2ch or 16ch, create a Multi-Output or Aggregate Device for speakers plus BlackHole, and rerun doctor. If COMLINK_SYSTEM_AUDIO_DEVICE is set, it must name a detected BlackHole input.".to_string(),
        ),
    };

    SystemAudioReport {
        available,
        status,
        strategy: "blackhole-virtual-audio-device".to_string(),
        detail,
        remediation,
        macos_version,
        dependency: SystemAudioDependency {
            name: "BlackHole virtual audio device".to_string(),
            present: dependency_present,
            device_name: matching_device,
            detail: "Chosen Phase 8 dependency for local Zoom/Teams system audio routing."
                .to_string(),
        },
        permissions: SystemAudioPermissions {
            microphone: PermissionDiagnostic {
                status: "manual-check".to_string(),
                detail: "Mic capture uses FFmpeg AVFoundation and requires the terminal or parent app to have macOS Microphone permission.".to_string(),
                remediation: "Grant Microphone permission in macOS Privacy & Security to the terminal or app launching Comlink; rerun `comlink meet start --source mic-only` as a smoke test.".to_string(),
            },
            system_audio_routing: PermissionDiagnostic {
                status: if dependency_present { "device-visible" } else { "setup-required" }.to_string(),
                detail: if dependency_present {
                    "BlackHole is visible as a local audio input; Comlink still depends on the user routing Teams/Zoom output into BlackHole.".to_string()
                } else {
                    "BlackHole is not visible as a local audio input, so system audio cannot be captured through the Phase 9 adapter.".to_string()
                },
                remediation: "Install BlackHole 2ch, include it in a Multi-Output or Aggregate Device with your speakers/headphones, then select that output in macOS or the meeting app.".to_string(),
            },
        },
        native_core_audio_tap: NativeCoreAudioTap {
            supported_by_os: native_supported,
            minimum_macos: MIN_NATIVE_TAP_MACOS.to_string(),
            permission_model: "Requires macOS system audio recording consent; doctor does not request or bypass TCC.".to_string(),
            detail: if native_supported {
                "Native Core Audio process taps are a future no-driver option, but are not the Phase 8 recommendation.".to_string()
            } else {
                "Native Core Audio process taps require a newer macOS than this probe observed.".to_string()
            },
        },
        source_metadata: SourceMetadataPrototype {
            labels: vec![
                "user_mic".to_string(),
                "system_audio".to_string(),
                "mixed".to_string(),
            ],
            modes: vec![
                "mic-only".to_string(),
                "system-only".to_string(),
                "mic-plus-system".to_string(),
            ],
            mixed_source_label: "mixed".to_string(),
        },
    }
}

fn preferred_device_name() -> Option<String> {
    env::var(SYSTEM_AUDIO_DEVICE_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn find_loopback_device(devices: &[String], preferred_device: Option<String>) -> Option<String> {
    if let Some(preferred) = preferred_device {
        if let Some(device) = devices
            .iter()
            .find(|device| device.eq_ignore_ascii_case(&preferred) && is_blackhole_device(device))
        {
            return Some(device.clone());
        }
    }

    devices
        .iter()
        .find(|device| is_blackhole_device(device))
        .cloned()
}

fn is_blackhole_device(device: &str) -> bool {
    device
        .to_ascii_lowercase()
        .contains(&BLACKHOLE_DEVICE_HINT.to_ascii_lowercase())
}

fn current_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOs(read_macos_version())
    } else {
        Platform::Other(env::consts::OS.to_string())
    }
}

fn read_macos_version() -> Option<MacOsVersion> {
    let output = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    MacOsVersion::parse(stdout.trim())
}

fn list_avfoundation_audio_devices(ffmpeg: &Path) -> Result<Vec<String>, String> {
    let output = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-f",
            "avfoundation",
            "-list_devices",
            "true",
            "-i",
            "",
        ])
        .output();

    let output = output
        .map_err(|error| format!("failed to run ffmpeg AVFoundation device probe: {error}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains("AVFoundation audio devices") {
        return Err(
            "ffmpeg AVFoundation device probe did not return an audio device list".to_string(),
        );
    }

    Ok(parse_avfoundation_audio_devices(&stderr))
}

fn parse_avfoundation_audio_devices(stderr: &str) -> Vec<String> {
    let mut devices = Vec::new();
    let mut in_audio = false;

    for line in stderr.lines() {
        if line.contains("AVFoundation audio devices") {
            in_audio = true;
            continue;
        }
        if line.contains("AVFoundation video devices") {
            in_audio = false;
            continue;
        }
        if !in_audio {
            continue;
        }
        if let Some(name) = line
            .split("] [")
            .nth(1)
            .and_then(|tail| tail.split("] ").nth(1))
        {
            devices.push(name.trim().to_string());
        }
    }

    devices
}

fn fake_snapshot(value: &str) -> SystemAudioProbeSnapshot {
    match value {
        "available" => SystemAudioProbeSnapshot {
            platform: Platform::MacOs(Some(MacOsVersion::new(14, 5, 0))),
            audio_input_devices: vec![
                "BlackHole 2ch".to_string(),
                "MacBook Pro Microphone".to_string(),
            ],
            probe_error: None,
        },
        "missing" | "unavailable" => SystemAudioProbeSnapshot {
            platform: Platform::MacOs(Some(MacOsVersion::new(14, 5, 0))),
            audio_input_devices: vec!["MacBook Pro Microphone".to_string()],
            probe_error: None,
        },
        "wrong-os" | "unsupported-os" => SystemAudioProbeSnapshot {
            platform: Platform::Other("linux".to_string()),
            audio_input_devices: Vec::new(),
            probe_error: None,
        },
        "probe-error" => SystemAudioProbeSnapshot {
            platform: Platform::MacOs(Some(MacOsVersion::new(14, 5, 0))),
            audio_input_devices: Vec::new(),
            probe_error: Some("fake AVFoundation device probe failed".to_string()),
        },
        _ => SystemAudioProbeSnapshot {
            platform: Platform::MacOs(Some(MacOsVersion::new(14, 5, 0))),
            audio_input_devices: Vec::new(),
            probe_error: Some(format!("unknown {SYSTEM_AUDIO_FAKE_ENV} value: {value}")),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacOsVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

impl MacOsVersion {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().unwrap_or("0").parse().ok()?;
        let patch = parts.next().unwrap_or("0").parse().ok()?;
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for MacOsVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_available_when_blackhole_input_is_present() {
        let report = report_from_snapshot(
            SystemAudioProbeSnapshot {
                platform: Platform::MacOs(Some(MacOsVersion::new(14, 6, 1))),
                audio_input_devices: vec![
                    "MacBook Pro Microphone".to_string(),
                    "BlackHole 2ch".to_string(),
                ],
                probe_error: None,
            },
            None,
        );

        assert!(report.available);
        assert_eq!(report.status, "ok");
        assert_eq!(
            report.dependency.device_name.as_deref(),
            Some("BlackHole 2ch")
        );
        assert!(report.native_core_audio_tap.supported_by_os);
        assert!(report
            .source_metadata
            .labels
            .contains(&"system_audio".to_string()));
    }

    #[test]
    fn reports_actionable_missing_dependency_on_macos() {
        let report = report_from_snapshot(
            SystemAudioProbeSnapshot {
                platform: Platform::MacOs(Some(MacOsVersion::new(14, 6, 1))),
                audio_input_devices: vec!["MacBook Pro Microphone".to_string()],
                probe_error: None,
            },
            None,
        );

        assert!(!report.available);
        assert_eq!(report.status, "missing-dependency");
        assert!(!report.dependency.present);
        assert!(report.remediation.contains("Install BlackHole"));
    }

    #[test]
    fn configured_device_does_not_mark_microphone_as_system_audio() {
        let report = report_from_snapshot(
            SystemAudioProbeSnapshot {
                platform: Platform::MacOs(Some(MacOsVersion::new(14, 6, 1))),
                audio_input_devices: vec!["MacBook Pro Microphone".to_string()],
                probe_error: None,
            },
            Some("MacBook Pro Microphone".to_string()),
        );

        assert!(!report.available);
        assert_eq!(report.status, "missing-dependency");
        assert!(!report.dependency.present);
        assert!(report.remediation.contains("COMLINK_SYSTEM_AUDIO_DEVICE"));
    }

    #[test]
    fn configured_blackhole_device_can_select_exact_input() {
        let report = report_from_snapshot(
            SystemAudioProbeSnapshot {
                platform: Platform::MacOs(Some(MacOsVersion::new(14, 6, 1))),
                audio_input_devices: vec![
                    "BlackHole 16ch".to_string(),
                    "BlackHole 2ch".to_string(),
                ],
                probe_error: None,
            },
            Some("BlackHole 16ch".to_string()),
        );

        assert!(report.available);
        assert_eq!(
            report.dependency.device_name.as_deref(),
            Some("BlackHole 16ch")
        );
    }

    #[test]
    fn capture_plan_uses_blackhole_as_avfoundation_audio_input() {
        struct Probe;

        impl SystemAudioProbe for Probe {
            fn snapshot(&self) -> SystemAudioProbeSnapshot {
                SystemAudioProbeSnapshot {
                    platform: Platform::MacOs(Some(MacOsVersion::new(14, 6, 1))),
                    audio_input_devices: vec![
                        "MacBook Pro Microphone".to_string(),
                        "BlackHole 2ch".to_string(),
                    ],
                    probe_error: None,
                }
            }
        }

        let plan = resolve_capture_plan(&Probe).unwrap();

        assert_eq!(plan.device_name, "BlackHole 2ch");
        assert_eq!(plan.avfoundation_input, ":BlackHole 2ch");
    }

    #[test]
    fn reports_probe_error_when_ffmpeg_output_has_no_audio_section() {
        let report = report_from_snapshot(
            SystemAudioProbeSnapshot {
                platform: Platform::MacOs(Some(MacOsVersion::new(14, 6, 1))),
                audio_input_devices: Vec::new(),
                probe_error: Some(
                    "ffmpeg AVFoundation device probe did not return an audio device list"
                        .to_string(),
                ),
            },
            None,
        );

        assert!(!report.available);
        assert_eq!(report.status, "probe-error");
        assert!(report.detail.contains("did not return"));
    }

    #[test]
    fn reports_wrong_os_without_requiring_host_audio() {
        let report = report_from_snapshot(
            SystemAudioProbeSnapshot {
                platform: Platform::Other("linux".to_string()),
                audio_input_devices: vec!["BlackHole 2ch".to_string()],
                probe_error: None,
            },
            None,
        );

        assert!(!report.available);
        assert_eq!(report.status, "unsupported-os");
        assert!(report.detail.contains("macOS"));
    }

    #[test]
    fn parses_avfoundation_audio_devices() {
        let stderr = "\
[AVFoundation indev @ 0x1] AVFoundation video devices:
[AVFoundation indev @ 0x1] [0] FaceTime HD Camera
[AVFoundation indev @ 0x1] AVFoundation audio devices:
[AVFoundation indev @ 0x1] [0] BlackHole 2ch
[AVFoundation indev @ 0x1] [1] MacBook Pro Microphone
";

        let devices = parse_avfoundation_audio_devices(stderr);

        assert_eq!(
            devices,
            vec![
                "BlackHole 2ch".to_string(),
                "MacBook Pro Microphone".to_string()
            ]
        );
    }

    #[test]
    fn parses_macos_versions_with_missing_patch() {
        assert_eq!(
            MacOsVersion::parse("14.4").unwrap(),
            MacOsVersion::new(14, 4, 0)
        );
        assert!(MacOsVersion::parse("not-a-version").is_none());
    }
}
