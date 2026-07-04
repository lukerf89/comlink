use std::{
    io::{self, Write},
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
    _tempdir: TempDir,
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
    stop_recorder(&mut child)?;

    let output = child
        .wait_with_output()
        .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?;

    if !output.status.success() || !wav_path.exists() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ComlinkError::AudioCaptureFailed(if stderr.is_empty() {
            "ffmpeg recorder exited without producing audio".to_string()
        } else {
            stderr
        }));
    }

    let duration_ms = audio::probe_duration_ms(&wav_path, options.ffprobe).unwrap_or(0);

    Ok(CapturedAudio {
        path: wav_path,
        duration_ms,
        _tempdir: tempdir,
    })
}

fn stop_recorder(child: &mut Child) -> Result<(), ComlinkError> {
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(b"q\n")
            .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?;
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
