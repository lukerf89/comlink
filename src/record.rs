use std::{
    fs::File,
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentedCapture {
    pub pid: u32,
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
    let output_pattern = options.chunks_dir.join("chunk-%05d.wav");

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
        .arg(output_pattern)
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

    Ok(SegmentedCapture { pid: child.id() })
}

pub fn stop_segmented_capture(pid: u32, timeout: Duration) -> Result<bool, ComlinkError> {
    stop_process_with_signal(pid, "INT")?;
    if wait_until_stopped(pid, timeout)? {
        return Ok(true);
    }

    stop_process_with_signal(pid, "TERM")?;
    if wait_until_stopped(pid, Duration::from_secs(2))? {
        return Ok(true);
    }

    stop_process_with_signal(pid, "KILL")?;
    wait_until_stopped(pid, Duration::from_secs(1))?;
    Ok(true)
}

pub fn process_is_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
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

fn stop_process_with_signal(pid: u32, signal: &str) -> Result<(), ComlinkError> {
    if !process_is_running(pid) {
        return Ok(());
    }

    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()
        .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?;

    if status.success() || !process_is_running(pid) {
        Ok(())
    } else {
        Err(ComlinkError::AudioCaptureFailed(format!(
            "failed to send SIG{signal} to recorder pid {pid}"
        )))
    }
}

fn wait_until_stopped(pid: u32, timeout: Duration) -> Result<bool, ComlinkError> {
    let started = Instant::now();
    loop {
        if !process_is_running(pid) {
            return Ok(true);
        }

        if started.elapsed() >= timeout {
            return Ok(false);
        }

        thread::sleep(Duration::from_millis(50));
    }
}
