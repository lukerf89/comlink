use std::{
    env,
    fs::File,
    io::{self, ErrorKind, Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::{
    audio::{self, AudioLevel},
    error::ComlinkError,
    system_audio,
};

const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Hardcoded AVFoundation selector used when neither an explicit device nor the
/// system default input can be resolved.
pub const DEFAULT_RECORD_DEVICE: &str = ":0";

/// Where the microphone capture device selector came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSource {
    /// `--device` on the command line.
    Flag,
    /// `COMLINK_RECORD_DEVICE`.
    Env,
    /// The CoreAudio system default input, mapped through the AVFoundation list.
    SystemDefault,
    /// The hardcoded `:0` fallback.
    Fallback,
}

impl DeviceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "--device",
            Self::Env => "COMLINK_RECORD_DEVICE",
            Self::SystemDefault => "system default input",
            Self::Fallback => "fallback",
        }
    }
}

/// A microphone capture device resolved with the same precedence `record` and
/// `meet start` use, so `doctor` reports exactly what capture will open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRecordDevice {
    /// Value passed to ffmpeg's AVFoundation `-i` (e.g. `:1`).
    pub avfoundation_input: String,
    /// Canonical device name when known.
    pub name: Option<String>,
    pub source: DeviceSource,
}

impl ResolvedRecordDevice {
    /// Human label: `Name (:N)` when the name is known, otherwise the selector.
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{name} ({})", self.avfoundation_input),
            None => self.avfoundation_input.clone(),
        }
    }
}

/// Pure precedence for an explicitly requested device: `--device` wins over
/// `COMLINK_RECORD_DEVICE`; blank values are ignored.
pub fn select_record_device_request(
    flag: Option<String>,
    env_value: Option<String>,
) -> Option<(String, DeviceSource)> {
    let normalize = |value: String| {
        let trimmed = value.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    };
    flag.and_then(normalize)
        .map(|value| (value, DeviceSource::Flag))
        .or_else(|| {
            env_value
                .and_then(normalize)
                .map(|value| (value, DeviceSource::Env))
        })
}

/// Resolve the microphone capture device for `record`, `meet start`, and
/// `doctor`.
///
/// Precedence: an explicit `--device`, then `COMLINK_RECORD_DEVICE`, then the
/// system (CoreAudio) default input device, then the hardcoded `:0` fallback.
/// Named selectors are mapped to their current AVFoundation index; numeric
/// selectors pass through. Prints nothing; callers own messaging.
pub fn resolve_record_device(
    requested: Option<String>,
    ffmpeg: &Path,
) -> Result<ResolvedRecordDevice, ComlinkError> {
    if let Some((selector, source)) =
        select_record_device_request(requested, env::var("COMLINK_RECORD_DEVICE").ok())
    {
        let matched = system_audio::resolve_capture_device(&selector, ffmpeg)
            .map_err(ComlinkError::AudioCaptureFailed)?;
        return Ok(ResolvedRecordDevice {
            avfoundation_input: matched.avfoundation_input,
            name: matched.name,
            source,
        });
    }

    if let Some(matched) = system_audio::resolve_default_input_device(ffmpeg) {
        return Ok(ResolvedRecordDevice {
            avfoundation_input: matched.avfoundation_input,
            name: matched.name,
            source: DeviceSource::SystemDefault,
        });
    }

    Ok(ResolvedRecordDevice {
        avfoundation_input: DEFAULT_RECORD_DEVICE.to_string(),
        name: None,
        source: DeviceSource::Fallback,
    })
}

/// Best-effort remediation hint for a near-silent capture on `device`. Lists
/// AVFoundation devices when ffmpeg can enumerate them.
pub fn near_silent_hint(device: &ResolvedRecordDevice, ffmpeg: &Path) -> String {
    let available = system_audio::list_avfoundation_audio_devices(ffmpeg).ok();
    audio::near_silent_device_hint(audio::DeviceHintContext {
        selector: &device.avfoundation_input,
        name: device.name.as_deref(),
        available: available.as_deref(),
    })
}

/// Pure: return the level only when it is near-silent. `None` (unmeasurable)
/// never flags, so an unreadable WAV keeps legacy behavior.
pub fn diagnose_record_level(level: Option<AudioLevel>) -> Option<AudioLevel> {
    level.filter(AudioLevel::is_near_silent)
}

/// Pure: an empty transcript from near-silent audio is almost always a device
/// or permission problem, so say so. Every other error passes through.
pub fn map_empty_transcript(
    error: ComlinkError,
    near_silent: Option<AudioLevel>,
    device: &str,
) -> ComlinkError {
    match (error, near_silent) {
        (ComlinkError::EmptyTranscript, Some(level)) => ComlinkError::NoSpeechNearSilent {
            mean_dbfs: level.mean_dbfs,
            device: device.to_string(),
        },
        (error, _) => error,
    }
}

/// Result of a short live microphone probe.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeOutcome {
    /// Capture succeeded and the WAV was measured.
    Level(AudioLevel),
    /// ffmpeg failed (non-zero exit, spawn failure, or no output file).
    CaptureFailed { stderr_tail: String },
    /// ffmpeg did not finish before the deadline; it was killed and reaped.
    TimedOut,
    /// Capture produced a file that could not be measured as 16-bit PCM.
    Unmeasurable,
}

/// Adapter seam for the `doctor --probe-mic` live capture so the doctor can be
/// tested with fakes and never touches AVFoundation unless asked.
pub trait MicProbe {
    fn probe(&self, device: &str) -> ProbeOutcome;
}

/// Maximum bytes of ffmpeg stderr kept for a probe diagnostic.
pub const PROBE_STDERR_CAP_BYTES: usize = 4 * 1024;

/// Real probe: captures a short WAV with ffmpeg AVFoundation into a temp dir,
/// bounded by a hard deadline (kill + reap), then measures its level.
#[derive(Debug, Clone)]
pub struct FfmpegMicProbe {
    pub ffmpeg: PathBuf,
    pub capture: Duration,
    pub deadline: Duration,
}

impl FfmpegMicProbe {
    pub fn new(ffmpeg: PathBuf) -> Self {
        Self {
            ffmpeg,
            capture: Duration::from_millis(1500),
            deadline: Duration::from_secs(5),
        }
    }
}

impl MicProbe for FfmpegMicProbe {
    fn probe(&self, device: &str) -> ProbeOutcome {
        self.probe_observed(device, |_| {})
    }
}

impl FfmpegMicProbe {
    /// [`MicProbe::probe`], reporting the spawned ffmpeg child's pid to
    /// `on_spawn` as soon as it exists (tests use it to prove the child is
    /// reaped without depending on the child running any code first).
    fn probe_observed(&self, device: &str, on_spawn: impl FnOnce(u32)) -> ProbeOutcome {
        // Dropped on every return path, which removes the probe WAV.
        let tempdir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(error) => {
                return ProbeOutcome::CaptureFailed {
                    stderr_tail: format!("could not create probe temp dir: {error}"),
                }
            }
        };
        let wav_path = tempdir.path().join("probe.wav");
        let seconds = format!("{:.3}", self.capture.as_secs_f64());

        let mut child = match Command::new(&self.ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "avfoundation",
                "-i",
            ])
            .arg(device)
            .args(["-t", &seconds])
            .args([
                "-ac",
                "1",
                "-ar",
                "16000",
                "-acodec",
                "pcm_s16le",
                "-f",
                "wav",
            ])
            .arg(&wav_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => {
                on_spawn(child.id());
                child
            }
            Err(error) => {
                return ProbeOutcome::CaptureFailed {
                    stderr_tail: format!("failed to start ffmpeg: {error}"),
                }
            }
        };

        let (sender, receiver) = mpsc::channel();
        let reader = child.stderr.take().map(|mut stderr| {
            thread::spawn(move || {
                let tail = read_capped_tail(&mut stderr, PROBE_STDERR_CAP_BYTES);
                let _ = sender.send(tail);
            })
        });

        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            if started.elapsed() >= self.deadline {
                break None;
            }
            thread::sleep(Duration::from_millis(20));
        };

        let timed_out = status.is_none();
        let status = match status {
            Some(status) => Some(status),
            None => {
                // Kill then reap so the probe never leaves a zombie behind.
                let _ = child.kill();
                child.wait().ok()
            }
        };

        // Grandchildren may keep the stderr pipe open after a kill; never block
        // on them. Join only once the reader has reported.
        let stderr_tail = match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(tail) => {
                if let Some(handle) = reader {
                    let _ = handle.join();
                }
                tail
            }
            Err(_) => String::new(),
        };

        if timed_out {
            return ProbeOutcome::TimedOut;
        }
        let Some(status) = status else {
            return ProbeOutcome::CaptureFailed { stderr_tail };
        };
        if !status.success() {
            let stderr_tail = if stderr_tail.is_empty() {
                format!("ffmpeg exited with {status}")
            } else {
                stderr_tail
            };
            return ProbeOutcome::CaptureFailed { stderr_tail };
        }
        if !wav_path.is_file() {
            return ProbeOutcome::CaptureFailed {
                stderr_tail: "ffmpeg produced no probe audio file".to_string(),
            };
        }

        match audio::read_wav_level_samples(&wav_path)
            .and_then(|samples| audio::session_audio_level([samples]))
        {
            Some(level) => ProbeOutcome::Level(level),
            None => ProbeOutcome::Unmeasurable,
        }
    }
}

/// Drain `reader` to EOF, keeping only the last `cap` bytes (lossy UTF-8,
/// trimmed). Keeps reading so the child never blocks on a full pipe.
fn read_capped_tail(reader: &mut impl Read, cap: usize) -> String {
    let mut kept: Vec<u8> = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                kept.extend_from_slice(&buffer[..read]);
                if kept.len() > cap {
                    let excess = kept.len() - cap;
                    kept.drain(..excess);
                }
            }
        }
    }
    String::from_utf8_lossy(&kept).trim().to_string()
}

#[derive(Debug)]
pub struct RecordingOptions<'a> {
    pub ffmpeg: &'a Path,
    pub ffprobe: Option<&'a Path>,
    pub device: &'a str,
}

#[derive(Debug)]
pub struct CapturedAudio {
    pub path: PathBuf,
    pub duration_ms: u64,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub stopped_at: Instant,
    _tempdir: TempDir,
}

#[derive(Debug)]
pub struct SegmentedCaptureOptions<'a> {
    pub ffmpeg: &'a Path,
    pub device: &'a str,
    pub chunks_dir: &'a Path,
    pub stderr_path: &'a Path,
    pub chunk_duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentedCapture {
    pub pid: u32,
    pub identity: SegmentedCaptureIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentedCaptureIdentity {
    pub pid: u32,
    pub output_pattern: String,
    pub process_started_at: Option<String>,
}

impl SegmentedCaptureIdentity {
    pub fn new(pid: u32, output_pattern: &Path) -> Self {
        Self {
            pid,
            output_pattern: output_pattern.display().to_string(),
            process_started_at: process_start_time(pid),
        }
    }
}

pub fn record_until_enter(options: RecordingOptions<'_>) -> Result<CapturedAudio, ComlinkError> {
    let tempdir = tempfile::tempdir()?;
    let wav_path = tempdir.path().join("recording.wav");

    let mut child = Command::new(options.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "avfoundation",
            "-i",
        ])
        .arg(options.device)
        .args(["-ac", "1", "-ar", "16000", "-f", "wav"])
        .arg(&wav_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?;

    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let stopped_at = Instant::now();
    stop_recorder(&mut child)?;

    let output = child
        .wait_with_output()
        .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?;

    if !output.status.success() || !wav_path.exists() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !stderr.is_empty() {
            return Err(ComlinkError::AudioCaptureFailed(stderr));
        }

        return Ok(CapturedAudio {
            path: wav_path,
            duration_ms: 0,
            sample_rate_hz: 16_000,
            channels: 1,
            stopped_at,
            _tempdir: tempdir,
        });
    }

    let duration_ms = audio::probe_duration_ms(&wav_path, options.ffprobe).unwrap_or(0);

    Ok(CapturedAudio {
        path: wav_path,
        duration_ms,
        sample_rate_hz: 16_000,
        channels: 1,
        stopped_at,
        _tempdir: tempdir,
    })
}

pub fn start_segmented_capture(
    options: SegmentedCaptureOptions<'_>,
) -> Result<SegmentedCapture, ComlinkError> {
    std::fs::create_dir_all(options.chunks_dir)?;
    if let Some(parent) = options.stderr_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let stderr = File::create(options.stderr_path)?;
    let chunk_seconds = options.chunk_duration.as_secs().max(1).to_string();
    let output_pattern = chunk_output_pattern(options.chunks_dir);

    let mut child = Command::new(options.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "avfoundation",
            "-i",
        ])
        .arg(options.device)
        .args(["-ac", "1", "-ar", "16000"])
        .args([
            "-f",
            "segment",
            "-segment_time",
            &chunk_seconds,
            "-reset_timestamps",
            "1",
            "-segment_format",
            "wav",
        ])
        .arg(&output_pattern)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        // Own process group: a meeting recorder outlives the process that
        // started it (`meet start`, or a `comlink mcp` server an MCP client
        // may restart), so a signal to the starter's group must not stop it.
        .process_group(0)
        .spawn()
        .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?;

    thread::sleep(Duration::from_millis(100));
    if let Some(status) = child
        .try_wait()
        .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?
    {
        let stderr = std::fs::read_to_string(options.stderr_path).unwrap_or_default();
        let detail = stderr.trim();
        return Err(ComlinkError::AudioCaptureFailed(if detail.is_empty() {
            format!("ffmpeg exited immediately with status {status}")
        } else {
            detail.to_string()
        }));
    }

    let pid = child.id();
    // Read the identity before the reaper can collect the child, so the pid
    // cannot have been reused yet.
    let identity = SegmentedCaptureIdentity::new(pid, &output_pattern);
    // Reap the recorder when it exits so a long-lived caller (the MCP server)
    // never accumulates zombie recorders. The thread only waits: stopping is
    // still done by signal through the recorded identity, and if this process
    // exits first the recorder is reparented and keeps running.
    if let Err(error) = thread::Builder::new()
        .name("comlink-recorder-reaper".to_string())
        .spawn(move || {
            let _ = child.wait();
        })
    {
        // Not fatal: the recorder runs either way, but it will linger as a
        // zombie after it exits until this process does.
        eprintln!("comlink: could not start the recorder reaper thread: {error}");
    }
    Ok(SegmentedCapture { pid, identity })
}

pub fn chunk_output_pattern(chunks_dir: &Path) -> PathBuf {
    chunks_dir.join("chunk-%05d.wav")
}

pub fn stop_segmented_capture(
    identity: &SegmentedCaptureIdentity,
    timeout: Duration,
) -> Result<bool, ComlinkError> {
    if !segmented_capture_is_running(identity) {
        return Ok(false);
    }

    stop_process_with_signal(identity, "INT")?;
    if wait_until_stopped(identity, timeout)? {
        return Ok(true);
    }

    stop_process_with_signal(identity, "TERM")?;
    if wait_until_stopped(identity, Duration::from_secs(2))? {
        return Ok(true);
    }

    stop_process_with_signal(identity, "KILL")?;
    if wait_until_stopped(identity, Duration::from_secs(1))? {
        Ok(true)
    } else {
        Err(ComlinkError::AudioCaptureFailed(format!(
            "recorder pid {} did not exit after SIGKILL",
            identity.pid
        )))
    }
}

/// A pid plus its OS-reported start time, so a recycled pid is never mistaken
/// for the process that was originally recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    #[serde(default)]
    pub process_started_at: Option<String>,
}

/// Capture the identity of a live pid (start time via `ps`).
pub fn process_identity(pid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        process_started_at: process_start_time(pid),
    }
}

/// Whether the identified process is still the same live process: the pid is
/// running, its start time matches (when one was recorded), and, when a
/// `command_needle` is given, its full command line contains that needle.
pub fn process_identity_is_running(
    identity: &ProcessIdentity,
    command_needle: Option<&str>,
) -> bool {
    if identity.pid == 0 || !process_is_running(identity.pid) {
        return false;
    }

    if let Some(expected) = identity.process_started_at.as_deref() {
        if process_start_time(identity.pid).as_deref() != Some(expected) {
            return false;
        }
    }

    match command_needle {
        Some(needle) => process_command(identity.pid)
            .map(|command| command.contains(needle))
            .unwrap_or(false),
        None => true,
    }
}

pub fn segmented_capture_is_running(identity: &SegmentedCaptureIdentity) -> bool {
    process_identity_is_running(
        &ProcessIdentity {
            pid: identity.pid,
            process_started_at: identity.process_started_at.clone(),
        },
        Some(&identity.output_pattern),
    )
}

pub fn process_is_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    system_command("/bin/kill", "kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn stop_recorder(child: &mut Child) -> Result<(), ComlinkError> {
    if child
        .try_wait()
        .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?
        .is_some()
    {
        child.stdin.take();
        return Ok(());
    }

    if let Some(stdin) = child.stdin.as_mut() {
        if let Err(error) = stdin.write_all(b"q\n") {
            if error.kind() != ErrorKind::BrokenPipe {
                return Err(ComlinkError::AudioCaptureFailed(error.to_string()));
            }
        }
    }
    child.stdin.take();

    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?
            .is_some()
        {
            return Ok(());
        }

        if started.elapsed() >= STOP_TIMEOUT {
            child
                .kill()
                .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?;
            return Ok(());
        }

        thread::sleep(Duration::from_millis(20));
    }
}

/// Signal the recorder's whole process group (recorders are started as group
/// leaders, so the group id is the recorded pid) so a wrapper's descendants
/// stop with it; fall back to the pid alone for a recorder that is not a
/// group leader (sessions started before recorders had their own group).
/// Only called after the leader was verified by identity, so the group is the
/// one the recorder created.
fn stop_process_with_signal(
    identity: &SegmentedCaptureIdentity,
    signal: &str,
) -> Result<(), ComlinkError> {
    let leader_running = segmented_capture_is_running(identity);
    if !leader_running && !recorder_group_alive(identity.pid) {
        return Ok(());
    }

    if send_signal(signal, &format!("-{}", identity.pid))? {
        return Ok(());
    }
    if !leader_running {
        return Ok(());
    }
    let status = send_signal(signal, &identity.pid.to_string())?;

    if status || !segmented_capture_is_running(identity) {
        Ok(())
    } else {
        Err(ComlinkError::AudioCaptureFailed(format!(
            "failed to send SIG{signal} to recorder pid {}",
            identity.pid
        )))
    }
}

/// `kill -<signal> -- <target>`; `target` is a pid, or `-<pgid>` for a
/// process group. Returns whether the signal was delivered.
fn send_signal(signal: &str, target: &str) -> Result<bool, ComlinkError> {
    system_command("/bin/kill", "kill")
        .arg(format!("-{signal}"))
        .arg("--")
        .arg(target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))
}

/// Whether any process is left in the recorder's process group (`kill -0` to
/// the group). False when the recorder never led a group.
fn recorder_group_alive(pgid: u32) -> bool {
    send_signal("0", &format!("-{pgid}")).unwrap_or(false)
}

/// Stopped means the leader is gone and so is every process in its group.
fn wait_until_stopped(
    identity: &SegmentedCaptureIdentity,
    timeout: Duration,
) -> Result<bool, ComlinkError> {
    let started = Instant::now();
    loop {
        if !segmented_capture_is_running(identity) && !recorder_group_alive(identity.pid) {
            return Ok(true);
        }

        if started.elapsed() >= timeout {
            return Ok(false);
        }

        thread::sleep(Duration::from_millis(50));
    }
}

fn process_start_time(pid: u32) -> Option<String> {
    let pid = pid.to_string();
    let output = system_command("/bin/ps", "ps")
        .args(["-p", &pid, "-o", "lstart="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn process_command(pid: u32) -> Option<String> {
    let pid = pid.to_string();
    let output = system_command("/bin/ps", "ps")
        .args(["-ww", "-p", &pid, "-o", "command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn system_command(absolute_path: &str, fallback_name: &str) -> Command {
    if Path::new(absolute_path).is_file() {
        Command::new(absolute_path)
    } else {
        Command::new(fallback_name)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn device_request_precedence_prefers_flag_then_env_and_ignores_blank() {
        assert_eq!(
            select_record_device_request(Some(":2".into()), Some(":1".into())),
            Some((":2".to_string(), DeviceSource::Flag))
        );
        assert_eq!(
            select_record_device_request(None, Some(" Studio Mic ".into())),
            Some(("Studio Mic".to_string(), DeviceSource::Env))
        );
        assert_eq!(
            select_record_device_request(Some("  ".into()), Some(":1".into())),
            Some((":1".to_string(), DeviceSource::Env))
        );
        assert_eq!(
            select_record_device_request(None, Some(String::new())),
            None
        );
        assert_eq!(select_record_device_request(None, None), None);
    }

    fn silent_level() -> AudioLevel {
        AudioLevel {
            mean_dbfs: -120.0,
            peak_dbfs: -120.0,
        }
    }

    fn speech_level() -> AudioLevel {
        AudioLevel {
            mean_dbfs: -25.0,
            peak_dbfs: -3.0,
        }
    }

    #[test]
    fn diagnose_record_level_flags_only_near_silent_measurements() {
        assert_eq!(
            diagnose_record_level(Some(silent_level())),
            Some(silent_level())
        );
        assert_eq!(diagnose_record_level(Some(speech_level())), None);
        assert_eq!(diagnose_record_level(None), None);
    }

    #[test]
    fn map_empty_transcript_upgrades_only_near_silent_empty_transcripts() {
        let mapped =
            map_empty_transcript(ComlinkError::EmptyTranscript, Some(silent_level()), ":0");
        match &mapped {
            ComlinkError::NoSpeechNearSilent { mean_dbfs, device } => {
                assert_eq!(*mean_dbfs, -120.0);
                assert_eq!(device, ":0");
            }
            other => panic!("expected NoSpeechNearSilent, got {other:?}"),
        }
        assert_eq!(mapped.exit_code(), 4);

        assert!(matches!(
            map_empty_transcript(ComlinkError::EmptyTranscript, None, ":0"),
            ComlinkError::EmptyTranscript
        ));
        assert!(matches!(
            map_empty_transcript(
                ComlinkError::WhisperFailed("boom".into()),
                Some(silent_level()),
                ":0"
            ),
            ComlinkError::WhisperFailed(_)
        ));
    }

    #[test]
    fn capped_tail_keeps_only_last_bytes() {
        let data = format!("{}END", "x".repeat(10_000));
        let tail = read_capped_tail(&mut data.as_bytes(), 16);
        assert_eq!(tail.len(), 16);
        assert!(tail.ends_with("END"));
    }

    #[cfg(unix)]
    fn mock_ffmpeg(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("mock-ffmpeg");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[cfg(unix)]
    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/audio")
            .join(name)
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_probe_times_out_kills_and_reaps_hanging_capture() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = mock_ffmpeg(dir.path(), "exec sleep 30");
        let probe = FfmpegMicProbe {
            ffmpeg,
            capture: Duration::from_millis(100),
            deadline: Duration::from_secs(2),
        };

        // The pid comes from the spawn itself, not from the mock writing a
        // pid file, so the assertion cannot race how far the child got
        // before the deadline kill (the old pid-file read flaked under load).
        let mut spawned_pid = None;
        let started = Instant::now();
        let outcome = probe.probe_observed(":0", |pid| spawned_pid = Some(pid));

        assert_eq!(outcome, ProbeOutcome::TimedOut);
        // The mock hangs for 30s: finishing well short of that proves the
        // deadline fired, without betting on scheduler latency under load.
        assert!(started.elapsed() < Duration::from_secs(15));
        let pid = spawned_pid.expect("probe did not report a spawned child");
        // A zombie still answers `kill -0`; a reaped pid does not.
        assert!(!process_is_running(pid), "probe child {pid} was not reaped");
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_probe_default_deadline_bounds_a_hang_with_orphaned_grandchild() {
        let dir = tempfile::tempdir().unwrap();
        // Non-exec sleep: the orphaned grandchild keeps the stderr pipe open.
        let ffmpeg = mock_ffmpeg(dir.path(), "sleep 30");
        let probe = FfmpegMicProbe::new(ffmpeg);

        let started = Instant::now();
        assert_eq!(probe.probe(":0"), ProbeOutcome::TimedOut);
        // 30s hang vs 5s deadline: well under 30s proves the bound held.
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_probe_reports_capture_failure_with_stderr_tail() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = mock_ffmpeg(
            dir.path(),
            "echo 'Input/output error: device :7 not found' >&2\nexit 3",
        );
        let outcome = FfmpegMicProbe::new(ffmpeg).probe(":7");
        match outcome {
            ProbeOutcome::CaptureFailed { stderr_tail } => {
                assert!(stderr_tail.contains("device :7 not found"), "{stderr_tail}")
            }
            other => panic!("expected CaptureFailed, got {other:?}"),
        }
    }

    #[cfg(unix)]
    fn copy_fixture_ffmpeg(dir: &Path, fixture_name: &str) -> PathBuf {
        // Copies the fixture to the last argument (the output WAV path).
        mock_ffmpeg(
            dir,
            &format!(
                "for last; do :; done\ncp '{}' \"$last\"",
                fixture(fixture_name).display()
            ),
        )
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_probe_measures_silence_as_near_silent_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let out_log = dir.path().join("out-path");
        let ffmpeg = mock_ffmpeg(
            dir.path(),
            &format!(
                "for last; do :; done\nprintf '%s' \"$last\" > '{}'\ncp '{}' \"$last\"",
                out_log.display(),
                fixture("silence.wav").display()
            ),
        );
        match FfmpegMicProbe::new(ffmpeg).probe(":0") {
            ProbeOutcome::Level(level) => assert!(level.is_near_silent(), "{level:?}"),
            other => panic!("expected Level, got {other:?}"),
        }
        let probe_wav = PathBuf::from(std::fs::read_to_string(&out_log).unwrap());
        assert!(probe_wav.ends_with("probe.wav"));
        assert!(!probe_wav.exists(), "probe WAV was not cleaned up");
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_probe_measures_signal_as_audible() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = copy_fixture_ffmpeg(dir.path(), "short.wav");
        match FfmpegMicProbe::new(ffmpeg).probe(":0") {
            ProbeOutcome::Level(level) => assert!(!level.is_near_silent(), "{level:?}"),
            other => panic!("expected Level, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_probe_reports_unmeasurable_output() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = mock_ffmpeg(
            dir.path(),
            "for last; do :; done\nprintf 'not a wav' > \"$last\"",
        );
        assert_eq!(
            FfmpegMicProbe::new(ffmpeg).probe(":0"),
            ProbeOutcome::Unmeasurable
        );
    }

    #[cfg(unix)]
    #[test]
    fn stop_segmented_capture_does_not_signal_pid_with_wrong_identity() {
        let mut child = system_command("/bin/sleep", "sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let identity = SegmentedCaptureIdentity {
            pid,
            output_pattern: "/tmp/comlink-not-this-recorder/chunk-%05d.wav".to_string(),
            process_started_at: process_start_time(pid),
        };

        let stopped = stop_segmented_capture(&identity, Duration::from_millis(10)).unwrap();

        assert!(!stopped);
        assert!(process_is_running(pid));

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn process_identity_requires_matching_start_time_and_command() {
        let mut child = system_command("/bin/sleep", "sleep")
            .arg("31")
            .spawn()
            .unwrap();
        let identity = process_identity(child.id());
        assert!(identity.process_started_at.is_some());
        assert!(process_identity_is_running(&identity, None));
        assert!(process_identity_is_running(&identity, Some("sleep 31")));
        assert!(!process_identity_is_running(
            &identity,
            Some("meet finalize not-this-session")
        ));

        let recycled = ProcessIdentity {
            pid: identity.pid,
            process_started_at: Some("Thu Jan  1 00:00:00 1970".to_string()),
        };
        assert!(!process_identity_is_running(&recycled, None));

        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!process_identity_is_running(&identity, None));
    }
}
