use std::{
    fs::File,
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::{audio, error::ComlinkError};

const STOP_TIMEOUT: Duration = Duration::from_secs(2);

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
    Ok(SegmentedCapture {
        pid,
        identity: SegmentedCaptureIdentity::new(pid, &output_pattern),
    })
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

pub fn segmented_capture_is_running(identity: &SegmentedCaptureIdentity) -> bool {
    if identity.pid == 0 || !process_is_running(identity.pid) {
        return false;
    }

    if let Some(expected) = identity.process_started_at.as_deref() {
        if process_start_time(identity.pid).as_deref() != Some(expected) {
            return false;
        }
    }

    process_command(identity.pid)
        .map(|command| command.contains(&identity.output_pattern))
        .unwrap_or(false)
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

fn stop_process_with_signal(
    identity: &SegmentedCaptureIdentity,
    signal: &str,
) -> Result<(), ComlinkError> {
    if !segmented_capture_is_running(identity) {
        return Ok(());
    }

    let status = system_command("/bin/kill", "kill")
        .arg(format!("-{signal}"))
        .arg(identity.pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?;

    if status.success() || !segmented_capture_is_running(identity) {
        Ok(())
    } else {
        Err(ComlinkError::AudioCaptureFailed(format!(
            "failed to send SIG{signal} to recorder pid {}",
            identity.pid
        )))
    }
}

fn wait_until_stopped(
    identity: &SegmentedCaptureIdentity,
    timeout: Duration,
) -> Result<bool, ComlinkError> {
    let started = Instant::now();
    loop {
        if !segmented_capture_is_running(identity) {
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
}
