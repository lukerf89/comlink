use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::TempDir;

use crate::error::ComlinkError;

#[derive(Debug)]
pub struct NormalizedAudio {
    pub path: PathBuf,
    pub duration_ms: u64,
    pub sample_rate_hz: u32,
    pub channels: u16,
    _tempdir: TempDir,
}

pub fn normalize_to_wav(
    input: &Path,
    ffmpeg: &Path,
    ffprobe: Option<&Path>,
) -> Result<NormalizedAudio, ComlinkError> {
    if !input.exists() {
        return Err(ComlinkError::InputMissing(input.to_path_buf()));
    }

    let tempdir = tempfile::tempdir()?;
    let wav_path = tempdir.path().join("normalized.wav");

    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "wav"])
        .arg(&wav_path)
        .output()?;

    if !output.status.success() {
        return Err(ComlinkError::FfmpegFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    let duration_ms = probe_duration_ms(&wav_path, ffprobe).unwrap_or(0);

    Ok(NormalizedAudio {
        path: wav_path,
        duration_ms,
        sample_rate_hz: 16_000,
        channels: 1,
        _tempdir: tempdir,
    })
}

pub fn probe_duration_ms(path: &Path, ffprobe: Option<&Path>) -> Option<u64> {
    if let Some(ffprobe) = ffprobe {
        if let Some(duration) = ffprobe_duration_ms(path, ffprobe) {
            return Some(duration);
        }
    }

    wav_duration_ms(path).ok()
}

fn ffprobe_duration_ms(path: &Path, ffprobe: &Path) -> Option<u64> {
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let seconds = stdout.trim().parse::<f64>().ok()?;
    Some((seconds * 1000.0).round() as u64)
}

fn wav_duration_ms(path: &Path) -> Result<u64, std::io::Error> {
    let mut file = File::open(path)?;
    let mut header = [0_u8; 12];
    file.read_exact(&mut header)?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Ok(0);
    }

    let mut byte_rate = None;
    let mut data_len = None;

    loop {
        let mut chunk_header = [0_u8; 8];
        if file.read_exact(&mut chunk_header).is_err() {
            break;
        }
        let id = &chunk_header[0..4];
        let len = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()) as u64;

        if id == b"fmt " {
            let mut fmt = vec![0_u8; len as usize];
            file.read_exact(&mut fmt)?;
            if fmt.len() >= 12 {
                byte_rate = Some(u32::from_le_bytes(fmt[8..12].try_into().unwrap()) as u64);
            }
        } else if id == b"data" {
            data_len = Some(len);
            file.seek(SeekFrom::Current(len as i64))?;
        } else {
            file.seek(SeekFrom::Current(len as i64))?;
        }

        if len % 2 == 1 {
            file.seek(SeekFrom::Current(1))?;
        }
    }

    match (byte_rate, data_len) {
        (Some(byte_rate), Some(data_len)) if byte_rate > 0 => Ok(data_len * 1000 / byte_rate),
        _ => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn reads_wav_duration_without_ffprobe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one-second.wav");
        let mut file = File::create(&path).unwrap();

        let sample_rate = 16_000_u32;
        let channels = 1_u16;
        let bits_per_sample = 16_u16;
        let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
        let data_len = byte_rate;

        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&channels.to_le_bytes()).unwrap();
        file.write_all(&sample_rate.to_le_bytes()).unwrap();
        file.write_all(&byte_rate.to_le_bytes()).unwrap();
        file.write_all(&(channels * bits_per_sample / 8).to_le_bytes())
            .unwrap();
        file.write_all(&bits_per_sample.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_len.to_le_bytes()).unwrap();
        file.write_all(&vec![0_u8; data_len as usize]).unwrap();

        assert_eq!(probe_duration_ms(&path, None), Some(1000));
    }
}
