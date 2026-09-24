#![cfg(unix)]
//! Process-level tests for LF-80: doctor stub-model detection, the opt-in
//! `doctor --probe-mic` live capture, and `record` near-silent diagnostics.

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

use serde_json::Value;

struct Harness {
    _tempdir: tempfile::TempDir,
    home: PathBuf,
    data: PathBuf,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    whisper: PathBuf,
    pbcopy: PathBuf,
    pbpaste: PathBuf,
    argv_log: PathBuf,
    model: PathBuf,
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

fn sparse_file(path: &Path, size: u64) {
    fs::File::create(path).unwrap().set_len(size).unwrap();
}

impl Harness {
    fn new() -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let root = tempdir.path().to_path_buf();
        let harness = Self {
            home: root.join("home"),
            data: root.join("data"),
            ffmpeg: root.join("mock-ffmpeg"),
            ffprobe: root.join("mock-ffprobe"),
            whisper: root.join("mock-whisper"),
            pbcopy: root.join("mock-pbcopy"),
            pbpaste: root.join("mock-pbpaste"),
            argv_log: root.join("ffmpeg-argv.log"),
            model: root.join("ggml-base.bin"),
            _tempdir: tempdir,
        };

        write_executable(
            &harness.ffmpeg,
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$COMLINK_MOCK_ARGV_LOG"

case " $* " in
  *" -list_devices true "*)
    {
      echo "[AVFoundation indev @ 0x1] AVFoundation video devices:"
      echo "[AVFoundation indev @ 0x1] [0] FaceTime HD Camera"
      echo "[AVFoundation indev @ 0x1] AVFoundation audio devices:"
      echo "[AVFoundation indev @ 0x1] [0] BlackHole 2ch"
      echo "[AVFoundation indev @ 0x1] [1] MacBook Pro Microphone"
    } >&2
    exit 1
    ;;
esac

out="${@: -1}"
case "${COMLINK_MOCK_CAPTURE_MODE:-copy}" in
  hang)
    sleep 30
    ;;
  fail)
    echo "mock avfoundation: Input/output error opening device" >&2
    exit 5
    ;;
  garbage)
    printf 'not a wav file' > "$out"
    ;;
  *)
    cp "$COMLINK_MOCK_CAPTURE_WAV" "$out"
    ;;
esac

# `record` stops the recorder by writing q to stdin; the probe uses -t.
read -r _ || true
exit 0
"#,
        );
        write_executable(&harness.ffprobe, "#!/usr/bin/env bash\nprintf '2.0\\n'\n");
        write_executable(
            &harness.whisper,
            r#"#!/usr/bin/env bash
set -euo pipefail
out=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -of) shift; out="$1" ;;
  esac
  shift || true
done
printf '%s' "${COMLINK_MOCK_TRANSCRIPT:-}" > "$out.txt"
"#,
        );
        write_executable(&harness.pbcopy, "#!/usr/bin/env bash\ncat > /dev/null\n");
        write_executable(&harness.pbpaste, "#!/usr/bin/env bash\nprintf ''\n");
        sparse_file(&harness.model, 16 << 20);
        fs::write(&harness.argv_log, "").unwrap();
        harness
    }

    fn command(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_comlink"));
        command
            .args(args)
            .env("COMLINK_HOME", &self.home)
            .env("COMLINK_DATA_DIR", &self.data)
            .env("COMLINK_FFMPEG", &self.ffmpeg)
            .env("COMLINK_FFPROBE", &self.ffprobe)
            .env("COMLINK_WHISPER_CPP", &self.whisper)
            .env("COMLINK_WHISPER_MODEL", &self.model)
            .env("COMLINK_PBCOPY", &self.pbcopy)
            .env("COMLINK_PBPASTE", &self.pbpaste)
            .env("COMLINK_LLM_ENABLED", "false")
            .env("COMLINK_RECORD_DEVICE", ":0")
            .env("COMLINK_MOCK_ARGV_LOG", &self.argv_log)
            .env("COMLINK_MOCK_CAPTURE_WAV", fixture("silence.wav"));
        for (key, value) in env {
            command.env(key, value);
        }
        command
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.command(args, env).output().unwrap()
    }

    fn record(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut child = self
            .command(args, env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Give the mock recorder a moment to write its capture, then stop.
        std::thread::sleep(Duration::from_millis(200));
        child.stdin.take().unwrap().write_all(b"\n").unwrap();
        child.wait_with_output().unwrap()
    }

    fn argv_lines(&self) -> Vec<String> {
        fs::read_to_string(&self.argv_log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON ({error}): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == name)
        .unwrap_or_else(|| panic!("doctor check {name} missing: {report}"))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn doctor_warns_on_stub_model_but_stays_ok_with_exit_zero() {
    let harness = Harness::new();
    fs::write(&harness.model, vec![0_u8; 1024]).unwrap();

    let output = harness.run(&["doctor", "--format", "json"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = stdout_json(&output);
    assert_eq!(report["ok"], true);
    let model = check(&report, "model-path");
    assert_eq!(model["status"], "warn");
    assert_eq!(model["required"], true);
    assert!(model["detail"].as_str().unwrap().contains("1024 bytes"));

    let text = harness.run(&["doctor"], &[]);
    assert_eq!(text.status.code(), Some(0));
    let text_stderr = stderr(&text);
    assert!(text_stderr.contains("[warn] model-path"), "{text_stderr}");
    assert!(text_stderr.contains("test stub"), "{text_stderr}");
}

#[test]
fn doctor_treats_large_model_as_ok_unless_named_for_tests() {
    let harness = Harness::new();
    let report = stdout_json(&harness.run(&["doctor", "--format", "json"], &[]));
    assert_eq!(check(&report, "model-path")["status"], "ok");

    let stub = harness.model.with_file_name("ggml-for-tests-x.bin");
    sparse_file(&stub, 16 << 20);
    let output = harness.run(
        &["doctor", "--format", "json"],
        &[("COMLINK_WHISPER_MODEL", stub.to_str().unwrap())],
    );
    assert_eq!(output.status.code(), Some(0));
    let report = stdout_json(&output);
    assert_eq!(check(&report, "model-path")["status"], "warn");
    assert_eq!(report["ok"], true);
}

#[test]
fn plain_doctor_never_captures_audio() {
    let harness = Harness::new();

    let output = harness.run(&["doctor", "--format", "json"], &[]);
    assert_eq!(output.status.code(), Some(0));
    let report = stdout_json(&output);
    let microphone = check(&report, "microphone");
    assert_eq!(microphone["status"], "info");
    assert!(microphone["remediation"]
        .as_str()
        .unwrap()
        .contains("--probe-mic"));
    for line in harness.argv_lines() {
        assert!(
            !line.contains("pcm_s16le") && !line.contains(" -t "),
            "plain doctor ran a capture: {line}"
        );
    }
}

#[test]
fn doctor_resolves_named_record_device_like_record_does() {
    let harness = Harness::new();

    let report = stdout_json(&harness.run(
        &["doctor", "--format", "json"],
        &[("COMLINK_RECORD_DEVICE", "MacBook Pro Microphone")],
    ));
    let detail = check(&report, "microphone")["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(detail.contains("MacBook Pro Microphone (:1)"), "{detail}");
}

#[test]
fn doctor_probe_mic_reports_silence_as_warn_without_failing() {
    let harness = Harness::new();

    let output = harness.run(&["doctor", "--format", "json", "--probe-mic"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(output.stderr.is_empty(), "{}", stderr(&output));
    let report = stdout_json(&output);
    assert_eq!(report["ok"], true);
    let microphone = check(&report, "microphone");
    assert_eq!(microphone["status"], "warn");
    assert_eq!(microphone["required"], false);
    assert!(microphone["detail"]
        .as_str()
        .unwrap()
        .contains("no signal from device"));
    assert!(harness
        .argv_lines()
        .iter()
        .any(|line| line.contains("pcm_s16le") && line.contains("-t 1.500")));
}

#[test]
fn doctor_probe_mic_reports_signal_as_ok() {
    let harness = Harness::new();
    let short = fixture("short.wav");

    let report = stdout_json(&harness.run(
        &["doctor", "--format", "json", "--probe-mic"],
        &[("COMLINK_MOCK_CAPTURE_WAV", short.to_str().unwrap())],
    ));
    assert_eq!(check(&report, "microphone")["status"], "ok");
}

#[test]
fn doctor_probe_mic_bounds_a_hanging_capture() {
    let harness = Harness::new();

    let started = Instant::now();
    let output = harness.run(
        &["doctor", "--format", "json", "--probe-mic"],
        &[("COMLINK_MOCK_CAPTURE_MODE", "hang")],
    );
    let elapsed = started.elapsed();

    assert_eq!(output.status.code(), Some(0));
    // The mock capture hangs for 30s against the 5s probe deadline. Finishing
    // well short of 30s proves the deadline fired; a tight bound here only
    // measured scheduler latency and flaked under parallel test load.
    assert!(elapsed < Duration::from_secs(15), "took {elapsed:?}");
    let report = stdout_json(&output);
    let microphone = check(&report, "microphone");
    assert_eq!(microphone["status"], "warn");
    assert!(microphone["detail"].as_str().unwrap().contains("timed out"));
}

#[test]
fn doctor_probe_mic_surfaces_capture_failure_stderr() {
    let harness = Harness::new();

    let output = harness.run(
        &["doctor", "--format", "json", "--probe-mic"],
        &[("COMLINK_MOCK_CAPTURE_MODE", "fail")],
    );
    assert_eq!(output.status.code(), Some(0));
    let report = stdout_json(&output);
    let microphone = check(&report, "microphone");
    assert_eq!(microphone["status"], "warn");
    assert!(microphone["detail"]
        .as_str()
        .unwrap()
        .contains("Input/output error opening device"));
}

#[test]
fn record_near_silent_with_text_warns_in_json_and_stderr() {
    let harness = Harness::new();

    let output = harness.record(
        &["record", "--format", "json", "--mode", "raw"],
        &[("COMLINK_MOCK_TRANSCRIPT", "you you you")],
    );
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(0), "{err}");
    let transcript = stdout_json(&output);
    let warnings = transcript["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("near-silent")),
        "{transcript}"
    );
    assert!(err.contains("near-silent"), "{err}");
    assert!(err.contains("COMLINK_RECORD_DEVICE"), "{err}");
    assert!(err.contains(":1 MacBook Pro Microphone"), "{err}");
}

#[test]
fn record_near_silent_with_empty_transcript_exits_four_with_hint() {
    let harness = Harness::new();

    let output = harness.record(&["record", "--format", "json"], &[]);
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(4), "{err}");
    assert!(output.stdout.is_empty());
    assert!(err.contains("no speech transcribed"), "{err}");
    assert!(err.contains("near-silent"), "{err}");
    assert!(err.contains("COMLINK_RECORD_DEVICE"), "{err}");
    assert!(err.contains("doctor --probe-mic"), "{err}");
}

#[test]
fn record_with_speech_level_has_no_near_silent_warning() {
    let harness = Harness::new();
    let short = fixture("short.wav");

    let output = harness.record(
        &["record", "--format", "json", "--mode", "raw"],
        &[
            ("COMLINK_MOCK_CAPTURE_WAV", short.to_str().unwrap()),
            ("COMLINK_MOCK_TRANSCRIPT", "Comlink phase zero fixture."),
        ],
    );
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(0), "{err}");
    let transcript = stdout_json(&output);
    assert!(transcript["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|warning| !warning.as_str().unwrap().contains("near-silent")));
    assert!(!err.contains("near-silent"), "{err}");
}

#[test]
fn record_unmeasurable_capture_keeps_legacy_empty_transcript_error() {
    let harness = Harness::new();

    let output = harness.record(
        &["record", "--format", "json"],
        &[("COMLINK_MOCK_CAPTURE_MODE", "garbage")],
    );
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(4), "{err}");
    assert!(
        err.contains("whisper.cpp produced no transcript text"),
        "{err}"
    );
    assert!(!err.contains("near-silent"), "{err}");
}
