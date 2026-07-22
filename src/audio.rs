use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
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

/// Mean full-scale level (dBFS) at or below which captured audio is treated as
/// effectively silent for the whole session.
///
/// Rationale: live speech usually sits around -30 dBFS, and even a quiet but
/// occupied room rarely averages below roughly -55 dBFS. A wrong/muted input
/// device or a missing microphone permission instead yields digital silence
/// approaching the noise floor (well below -80 dBFS). -60 dBFS leaves headroom
/// above genuine quiet speech while still catching dead inputs — the exact case
/// that makes whisper.cpp hallucinate filler like "you you you you". This sits
/// inside the -60..-70 dBFS band the issue calls out and errs toward fewer
/// false positives on real-but-quiet rooms.
pub const NEAR_SILENT_MEAN_DBFS_THRESHOLD: f64 = -60.0;

/// Reported floor for dBFS values so digital silence (amplitude 0) stays a
/// finite, JSON-serializable number instead of -inf.
pub const SILENCE_FLOOR_DBFS: f64 = -120.0;

/// Raw energy accumulated from a single WAV file, kept separate from the dBFS
/// conversion so per-chunk measurements can be combined into a session total
/// with correct (energy-weighted) averaging.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WavLevelSamples {
    pub sum_squares: f64,
    pub peak_abs: f64,
    pub sample_count: u64,
}

/// Session-level audio level, in dBFS relative to 16-bit full scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioLevel {
    pub mean_dbfs: f64,
    pub peak_dbfs: f64,
}

impl AudioLevel {
    /// Whether the mean level is at or below the near-silent threshold.
    pub fn is_near_silent(&self) -> bool {
        is_near_silent(self.mean_dbfs, NEAR_SILENT_MEAN_DBFS_THRESHOLD)
    }
}

/// Pure classifier: is a measured mean dBFS at/below the near-silent threshold?
/// The boundary (mean exactly equal to the threshold) counts as near-silent.
pub fn is_near_silent(mean_dbfs: f64, threshold: f64) -> bool {
    mean_dbfs <= threshold
}

/// Combine per-file energy measurements into an overall session level. Returns
/// `None` when no samples were measured (nothing to classify).
pub fn session_audio_level(
    measurements: impl IntoIterator<Item = WavLevelSamples>,
) -> Option<AudioLevel> {
    let mut sum_squares = 0.0_f64;
    let mut peak_abs = 0.0_f64;
    let mut sample_count = 0_u64;
    for measurement in measurements {
        sum_squares += measurement.sum_squares;
        peak_abs = peak_abs.max(measurement.peak_abs);
        sample_count = sample_count.saturating_add(measurement.sample_count);
    }
    if sample_count == 0 {
        return None;
    }
    let rms = (sum_squares / sample_count as f64).sqrt();
    Some(AudioLevel {
        mean_dbfs: amplitude_to_dbfs(rms),
        peak_dbfs: amplitude_to_dbfs(peak_abs),
    })
}

/// Human-facing warning for a near-silent session. Ties remediation to the
/// device/permission/mute causes and points at `comlink doctor`, which reports
/// microphone permission as a manual-check.
pub fn near_silent_warning_message(mean_dbfs: f64) -> String {
    format!(
        "captured audio was near-silent ({mean_dbfs:.1} dBFS avg); check that the intended input device is selected and not muted, and that microphone permission is granted (see `comlink doctor`); transcript may be unreliable"
    )
}

/// Read a 16-bit PCM WAV and accumulate its energy (sum of squares), peak
/// amplitude, and sample count. Returns `None` when the file is missing,
/// unreadable, or not 16-bit PCM (so measurement is best-effort and never a
/// hard failure). Channel count is ignored — every sample contributes to the
/// level regardless of interleaving.
pub fn read_wav_level_samples(path: &Path) -> Option<WavLevelSamples> {
    // Buffered so the sample-by-sample reads below don't become one syscall per
    // 16-bit sample (millions per chunk) on the `meet stop` latency path.
    let mut file = BufReader::new(File::open(path).ok()?);
    let mut header = [0_u8; 12];
    file.read_exact(&mut header).ok()?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return None;
    }

    let mut bits_per_sample: Option<u16> = None;
    let mut sum_squares = 0.0_f64;
    let mut peak_abs = 0.0_f64;
    let mut sample_count = 0_u64;

    loop {
        let mut chunk_header = [0_u8; 8];
        if file.read_exact(&mut chunk_header).is_err() {
            break;
        }
        let id = [
            chunk_header[0],
            chunk_header[1],
            chunk_header[2],
            chunk_header[3],
        ];
        let len = u32::from_le_bytes(chunk_header[4..8].try_into().ok()?) as u64;

        if &id == b"fmt " {
            let mut fmt = vec![0_u8; len as usize];
            file.read_exact(&mut fmt).ok()?;
            if fmt.len() >= 16 {
                bits_per_sample = Some(u16::from_le_bytes(fmt[14..16].try_into().ok()?));
            }
        } else if &id == b"data" {
            // Only 16-bit PCM is produced by the capture pipeline; bail out on
            // anything else rather than mis-measuring the level.
            if bits_per_sample != Some(16) {
                return None;
            }
            let mut remaining = len;
            let mut sample = [0_u8; 2];
            while remaining >= 2 {
                if file.read_exact(&mut sample).is_err() {
                    break;
                }
                let value = i16::from_le_bytes(sample) as f64;
                sum_squares += value * value;
                peak_abs = peak_abs.max(value.abs());
                sample_count += 1;
                remaining -= 2;
            }
            if remaining > 0 {
                let _ = file.seek(SeekFrom::Current(remaining as i64));
            }
        } else {
            file.seek(SeekFrom::Current(len as i64)).ok()?;
        }

        if len % 2 == 1 {
            file.seek(SeekFrom::Current(1)).ok()?;
        }
    }

    (sample_count > 0).then_some(WavLevelSamples {
        sum_squares,
        peak_abs,
        sample_count,
    })
}

fn amplitude_to_dbfs(amplitude: f64) -> f64 {
    const FULL_SCALE: f64 = 32_768.0;
    if amplitude <= 0.0 {
        return SILENCE_FLOOR_DBFS;
    }
    (20.0 * (amplitude / FULL_SCALE).log10()).max(SILENCE_FLOOR_DBFS)
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

    fn write_pcm16_wav(path: &Path, samples: &[i16]) {
        let sample_rate = 16_000_u32;
        let channels = 1_u16;
        let bits_per_sample = 16_u16;
        let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
        let data_len = (samples.len() * 2) as u32;

        let mut file = File::create(path).unwrap();
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
        for sample in samples {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
    }

    #[test]
    fn classifier_flags_below_threshold_and_boundary_but_not_above() {
        // Below the threshold => near-silent.
        assert!(is_near_silent(-70.0, NEAR_SILENT_MEAN_DBFS_THRESHOLD));
        // Exactly at the boundary => near-silent.
        assert!(is_near_silent(
            NEAR_SILENT_MEAN_DBFS_THRESHOLD,
            NEAR_SILENT_MEAN_DBFS_THRESHOLD
        ));
        // Above the threshold (louder) => not near-silent.
        assert!(!is_near_silent(-59.9, NEAR_SILENT_MEAN_DBFS_THRESHOLD));
        assert!(!is_near_silent(-30.0, NEAR_SILENT_MEAN_DBFS_THRESHOLD));
    }

    #[test]
    fn digital_silence_reads_as_near_silent_at_the_floor() {
        let level = session_audio_level([WavLevelSamples {
            sum_squares: 0.0,
            peak_abs: 0.0,
            sample_count: 16_000,
        }])
        .expect("some samples");
        assert_eq!(level.mean_dbfs, SILENCE_FLOOR_DBFS);
        assert!(level.is_near_silent());
    }

    #[test]
    fn loud_tone_is_not_near_silent() {
        // Constant near-full-scale amplitude => ~0 dBFS.
        let level = session_audio_level([WavLevelSamples {
            sum_squares: 30_000.0 * 30_000.0 * 1_000.0,
            peak_abs: 30_000.0,
            sample_count: 1_000,
        }])
        .expect("some samples");
        assert!(level.mean_dbfs > -6.0, "mean was {}", level.mean_dbfs);
        assert!(!level.is_near_silent());
    }

    #[test]
    fn session_level_energy_weights_across_chunks() {
        let empty = session_audio_level(std::iter::empty());
        assert!(empty.is_none());

        let loud = WavLevelSamples {
            sum_squares: 20_000.0 * 20_000.0 * 100.0,
            peak_abs: 20_000.0,
            sample_count: 100,
        };
        let silent = WavLevelSamples {
            sum_squares: 0.0,
            peak_abs: 0.0,
            sample_count: 100,
        };
        let combined = session_audio_level([loud, silent]).expect("some samples");
        // Half loud, half silent: RMS = 20000/sqrt(2) ~= 14142 => ~ -7.3 dBFS.
        assert!(combined.mean_dbfs > -9.0 && combined.mean_dbfs < -5.0);
        assert!(!combined.is_near_silent());
    }

    #[test]
    fn reads_level_from_pcm16_wav_and_measures_silence() {
        let dir = tempfile::tempdir().unwrap();

        let silent_path = dir.path().join("silent.wav");
        write_pcm16_wav(&silent_path, &vec![0_i16; 16_000]);
        let silent = read_wav_level_samples(&silent_path).expect("silent samples");
        assert_eq!(silent.sample_count, 16_000);
        assert_eq!(silent.sum_squares, 0.0);
        let silent_level = session_audio_level([silent]).expect("level");
        assert!(silent_level.is_near_silent());

        let loud_path = dir.path().join("loud.wav");
        write_pcm16_wav(&loud_path, &vec![20_000_i16; 16_000]);
        let loud = read_wav_level_samples(&loud_path).expect("loud samples");
        assert_eq!(loud.peak_abs, 20_000.0);
        let loud_level = session_audio_level([loud]).expect("level");
        assert!(!loud_level.is_near_silent());
    }

    #[test]
    fn near_silent_warning_mentions_device_permission_and_doctor() {
        let level = AudioLevel {
            mean_dbfs: -70.0,
            peak_dbfs: -60.0,
        };
        let message = near_silent_warning_message(level.mean_dbfs);
        assert!(message.contains("near-silent"));
        assert!(message.contains("-70.0 dBFS"));
        assert!(message.contains("device"));
        assert!(message.contains("permission"));
        assert!(message.contains("muted"));
        assert!(message.contains("comlink doctor"));
    }
}
