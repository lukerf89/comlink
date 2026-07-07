use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use crate::error::ComlinkError;

const WHISPER_QUALITY_ARGS: &[&str] = &["--suppress-nst", "--no-fallback", "--temperature", "0"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceMetadata {
    pub path: String,
    pub normalized_sample_rate_hz: u32,
    pub normalized_channels: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct Transcript {
    pub text: String,
    pub engine: String,
    pub model: String,
    pub duration_ms: u64,
    pub segments: Vec<Segment>,
    pub source: SourceMetadata,
}

pub trait AsrEngine {
    fn transcribe(
        &self,
        wav_path: &Path,
        source: SourceMetadata,
        duration_ms: u64,
    ) -> Result<Transcript, ComlinkError>;
}

#[derive(Debug, Clone)]
pub struct WhisperCppEngine {
    pub binary: PathBuf,
    pub model: PathBuf,
}

impl AsrEngine for WhisperCppEngine {
    fn transcribe(
        &self,
        wav_path: &Path,
        source: SourceMetadata,
        duration_ms: u64,
    ) -> Result<Transcript, ComlinkError> {
        let output_dir = tempfile::tempdir()?;
        let output_base = output_dir.path().join("transcript");

        let output = self.run_whisper_with_quality_fallback(wav_path, &output_base, false)?;
        let output = if !output.status.success() && should_retry_without_gpu(&output.stderr) {
            let retry = self.run_whisper_with_quality_fallback(wav_path, &output_base, true)?;
            if retry.status.success() {
                retry
            } else {
                return Err(ComlinkError::WhisperFailed(combine_whisper_errors(
                    &output.stderr,
                    &retry.stderr,
                )));
            }
        } else {
            output
        };

        if !output.status.success() {
            return Err(ComlinkError::WhisperFailed(trim_for_error(&output.stderr)));
        }

        let text_path = output_base.with_extension("txt");
        let text = if text_path.exists() {
            fs::read_to_string(text_path)?
        } else {
            String::from_utf8_lossy(&output.stdout).to_string()
        };
        let text = clean_whisper_text(&text);

        if text.is_empty() {
            return Err(ComlinkError::EmptyTranscript);
        }

        Ok(Transcript {
            segments: vec![Segment {
                start_ms: 0,
                end_ms: duration_ms,
                text: text.clone(),
            }],
            text,
            engine: "whisper.cpp".to_string(),
            model: self.model.display().to_string(),
            duration_ms,
            source,
        })
    }
}

impl WhisperCppEngine {
    fn run_whisper_with_quality_fallback(
        &self,
        wav_path: &Path,
        output_base: &Path,
        no_gpu: bool,
    ) -> Result<std::process::Output, ComlinkError> {
        let output = self.run_whisper(wav_path, output_base, no_gpu, true)?;
        if !output.status.success() && should_retry_without_quality_args(&output.stderr) {
            self.run_whisper(wav_path, output_base, no_gpu, false)
        } else {
            Ok(output)
        }
    }

    fn run_whisper(
        &self,
        wav_path: &Path,
        output_base: &Path,
        no_gpu: bool,
        quality_args: bool,
    ) -> Result<std::process::Output, ComlinkError> {
        let mut command = Command::new(&self.binary);
        command.arg("-m").arg(&self.model).arg("-f").arg(wav_path);
        if no_gpu {
            command.arg("-ng");
        }
        if quality_args {
            command.args(WHISPER_QUALITY_ARGS);
        }
        command.args(["-otxt", "-of"]).arg(output_base);
        Ok(command.output()?)
    }
}

fn clean_whisper_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn trim_for_error(bytes: &[u8]) -> String {
    let message = String::from_utf8_lossy(bytes).trim().to_string();
    if message.is_empty() {
        "process exited without an error message".to_string()
    } else {
        message
    }
}

fn should_retry_without_gpu(stderr: &[u8]) -> bool {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    message.contains("metal")
        || message.contains("gpu")
        || message.contains("ggml_metal")
        || message.contains("failed to allocate buffer")
}

fn should_retry_without_quality_args(stderr: &[u8]) -> bool {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    message.contains("unknown argument")
        || message.contains("unknown option")
        || message.contains("invalid argument")
        || message.contains("unrecognized option")
}

fn combine_whisper_errors(first: &[u8], retry: &[u8]) -> String {
    format!(
        "initial GPU attempt failed: {}; CPU retry failed: {}",
        trim_for_error(first),
        trim_for_error(retry)
    )
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use super::*;

    #[cfg(unix)]
    #[test]
    fn whisper_cpp_adapter_reads_txt_output() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock-whisper");
        let model = dir.path().join("model.bin");
        let wav = dir.path().join("audio.wav");
        fs::write(&model, "model").unwrap();
        fs::write(&wav, "wav").unwrap();

        let mut file = fs::File::create(&script).unwrap();
        writeln!(
            file,
            r#"#!/bin/sh
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-of" ]; then
    shift
    out="$1"
  fi
  shift
done
printf 'hello from mock whisper\n' > "$out.txt"
"#
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let engine = WhisperCppEngine {
            binary: script,
            model: model.clone(),
        };

        let transcript = engine
            .transcribe(
                &wav,
                SourceMetadata {
                    path: "input.wav".to_string(),
                    normalized_sample_rate_hz: 16_000,
                    normalized_channels: 1,
                },
                1234,
            )
            .unwrap();

        assert_eq!(transcript.text, "hello from mock whisper");
        assert_eq!(transcript.engine, "whisper.cpp");
        assert_eq!(transcript.model, model.display().to_string());
        assert_eq!(transcript.duration_ms, 1234);
        assert_eq!(transcript.segments[0].end_ms, 1234);
    }

    #[cfg(unix)]
    #[test]
    fn whisper_cpp_adapter_passes_quality_args() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock-whisper");
        let args_path = dir.path().join("args.txt");
        let model = dir.path().join("model.bin");
        let wav = dir.path().join("audio.wav");
        fs::write(&model, "model").unwrap();
        fs::write(&wav, "wav").unwrap();

        let mut file = fs::File::create(&script).unwrap();
        writeln!(
            file,
            r#"#!/bin/sh
printf '%s\n' "$@" > "{}"
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-of" ]; then
    shift
    out="$1"
  fi
  shift
done
printf 'hello from mock whisper\n' > "$out.txt"
"#,
            args_path.display()
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let engine = WhisperCppEngine {
            binary: script,
            model: model.clone(),
        };

        engine
            .transcribe(
                &wav,
                SourceMetadata {
                    path: "input.wav".to_string(),
                    normalized_sample_rate_hz: 16_000,
                    normalized_channels: 1,
                },
                1234,
            )
            .unwrap();

        let args = fs::read_to_string(args_path).unwrap();
        assert!(args.contains("--suppress-nst\n"));
        assert!(args.contains("--no-fallback\n"));
        assert!(args.contains("--temperature\n0\n"));
    }

    #[cfg(unix)]
    #[test]
    fn whisper_cpp_adapter_retries_without_quality_args_when_unsupported() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock-whisper");
        let model = dir.path().join("model.bin");
        let wav = dir.path().join("audio.wav");
        let calls = dir.path().join("calls.txt");
        fs::write(&model, "model").unwrap();
        fs::write(&wav, "wav").unwrap();

        let mut file = fs::File::create(&script).unwrap();
        writeln!(
            file,
            r#"#!/bin/sh
printf 'call\n' >> "{}"
for arg in "$@"; do
  if [ "$arg" = "--suppress-nst" ]; then
    echo "unknown argument: --suppress-nst" >&2
    exit 2
  fi
done
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-of" ]; then
    shift
    out="$1"
  fi
  shift
done
printf 'fallback transcript\n' > "$out.txt"
"#,
            calls.display()
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let engine = WhisperCppEngine {
            binary: script,
            model: model.clone(),
        };

        let transcript = engine
            .transcribe(
                &wav,
                SourceMetadata {
                    path: "input.wav".to_string(),
                    normalized_sample_rate_hz: 16_000,
                    normalized_channels: 1,
                },
                1234,
            )
            .unwrap();

        assert_eq!(transcript.text, "fallback transcript");
        assert_eq!(fs::read_to_string(calls).unwrap().lines().count(), 2);
    }

    #[test]
    fn cpu_retry_is_limited_to_gpu_backend_failures() {
        assert!(should_retry_without_gpu(
            b"ggml_metal_buffer_init: error: failed to allocate buffer"
        ));
        assert!(should_retry_without_gpu(b"GPU device failed"));
        assert!(!should_retry_without_gpu(b"model path does not exist"));
        assert!(!should_retry_without_gpu(b"invalid audio data"));
    }

    #[test]
    fn combined_whisper_error_preserves_initial_failure_context() {
        let message = combine_whisper_errors(b"metal allocation failed", b"cpu decode failed");

        assert!(message.contains("initial GPU attempt failed: metal allocation failed"));
        assert!(message.contains("CPU retry failed: cpu decode failed"));
    }
}
