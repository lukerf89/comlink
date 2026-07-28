#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::Duration,
};

use serde_json::Value;

struct MockRuntime {
    _tempdir: tempfile::TempDir,
    home: PathBuf,
    data: PathBuf,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    whisper: PathBuf,
    model: PathBuf,
}

impl MockRuntime {
    fn new() -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let root = tempdir.path().to_path_buf();
        let runtime = Self {
            home: root.join("home"),
            data: root.join("data"),
            ffmpeg: root.join("mock-ffmpeg"),
            ffprobe: root.join("mock-ffprobe"),
            whisper: root.join("mock-whisper"),
            model: root.join("model.bin"),
            _tempdir: tempdir,
        };

        write_executable(
            &runtime.ffmpeg,
            r#"#!/usr/bin/env bash
set -euo pipefail

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
input=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -i)
      shift
      input="${1:-}"
      ;;
  esac
  shift || true
done

if [ -n "${COMLINK_MOCK_FAIL_INPUT:-}" ] && [[ "$input" == *"$COMLINK_MOCK_FAIL_INPUT"* ]]; then
  echo "mock ffmpeg failed for input $input" >&2
  exit 8
fi

chunks="${COMLINK_MOCK_MEETING_CHUNKS:-4}"
mkdir -p "$(dirname "$out")"
if [ "$chunks" -gt 0 ]; then
  for index in $(seq 0 $((chunks - 1))); do
    chunk="$(printf "$out" "$index")"
    printf 'mock wav %s\n' "$index" > "$chunk"
  done
fi

trap 'exit 0' INT TERM
while true; do
  sleep 1
done
"#,
        );
        write_executable(
            &runtime.ffprobe,
            r#"#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "${COMLINK_MOCK_DURATION_SECONDS:-30}"
"#,
        );
        write_executable(
            &runtime.whisper,
            r#"#!/usr/bin/env bash
set -euo pipefail

out=""
wav=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -of)
      shift
      out="$1"
      ;;
    -f)
      shift
      wav="$1"
      ;;
  esac
  shift || true
done

if [ -z "$out" ]; then
  echo "missing -of" >&2
  exit 2
fi

name="$(basename "$wav" .wav)"
source_label="$(basename "$(dirname "$wav")")"
if [ "$source_label" = "chunks" ]; then
  source_label="Meeting"
fi
index="${name#chunk-}"
fail=",${COMLINK_MOCK_FAIL_CHUNKS:-},"
if [[ "$fail" == *",$index,"* ]]; then
  echo "mock whisper failed for $index" >&2
  exit 7
fi
silent=",${COMLINK_MOCK_SILENT_CHUNKS:-},"
if [[ "$silent" == *",$index,"* ]]; then
  : > "$out.txt"
else
  printf '%s segment %s.\n' "$source_label" "$index" > "$out.txt"
fi
"#,
        );
        fs::write(&runtime.model, "mock model\n").unwrap();

        runtime
    }

    fn run(&self, args: &[&str], chunks: u32, silent_chunks: &str) -> Output {
        self.run_with_failures(args, chunks, silent_chunks, "")
    }

    fn run_with_failures(
        &self,
        args: &[&str],
        chunks: u32,
        silent_chunks: &str,
        fail_chunks: &str,
    ) -> Output {
        Command::new(env!("CARGO_BIN_EXE_comlink"))
            .args(args)
            .env("COMLINK_HOME", &self.home)
            .env("COMLINK_DATA_DIR", &self.data)
            .env("COMLINK_FFMPEG", &self.ffmpeg)
            .env("COMLINK_FFPROBE", &self.ffprobe)
            .env("COMLINK_WHISPER_CPP", &self.whisper)
            .env("COMLINK_WHISPER_MODEL", &self.model)
            .env("COMLINK_LLM_ENABLED", "false")
            .env("COMLINK_RECORD_DEVICE", ":0")
            .env("COMLINK_MOCK_MEETING_CHUNKS", chunks.to_string())
            .env("COMLINK_MOCK_DURATION_SECONDS", "30")
            .env("COMLINK_MOCK_SILENT_CHUNKS", silent_chunks)
            .env("COMLINK_MOCK_FAIL_CHUNKS", fail_chunks)
            .output()
            .unwrap()
    }

    fn run_with_failed_input(&self, args: &[&str], fail_input: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_comlink"))
            .args(args)
            .env("COMLINK_HOME", &self.home)
            .env("COMLINK_DATA_DIR", &self.data)
            .env("COMLINK_FFMPEG", &self.ffmpeg)
            .env("COMLINK_FFPROBE", &self.ffprobe)
            .env("COMLINK_WHISPER_CPP", &self.whisper)
            .env("COMLINK_WHISPER_MODEL", &self.model)
            .env("COMLINK_LLM_ENABLED", "false")
            .env("COMLINK_RECORD_DEVICE", ":0")
            .env("COMLINK_MOCK_FAIL_INPUT", fail_input)
            .output()
            .unwrap()
    }
}

#[test]
fn meet_stop_writes_exports_when_middle_and_final_chunks_are_empty() {
    let runtime = MockRuntime::new();

    let start = runtime.run(
        &[
            "meet",
            "start",
            "--format",
            "json",
            "--chunk-seconds",
            "30",
            "--no-llm",
        ],
        4,
        "00001,00003",
    );
    assert_success(&start);
    let start_json = json_stdout(&start);
    wait_for_chunks(Path::new(start_json["chunks_dir"].as_str().unwrap()), 4);

    let session_id = start_json["session_id"].as_str().unwrap();
    let stop = runtime.run(
        &["meet", "stop", session_id, "--format", "json"],
        4,
        "00001,00003",
    );
    assert_success(&stop);
    let stop_json = json_stdout(&stop);

    assert_eq!(stop_json["status"], "stopped");
    assert_eq!(stop_json["chunks_processed"], 4);
    assert_eq!(stop_json["segment_count"], 2);
    assert_eq!(stop_json["duration_ms"], 120_000);

    let artifacts = &stop_json["artifacts"];
    let json_export_path = Path::new(artifacts["json_export"].as_str().unwrap());
    let markdown_export_path = Path::new(artifacts["markdown_export"].as_str().unwrap());
    let segments_jsonl_path = Path::new(artifacts["segments_jsonl"].as_str().unwrap());
    assert!(json_export_path.is_file());
    assert!(markdown_export_path.is_file());
    assert!(segments_jsonl_path.is_file());

    let export: Value = serde_json::from_slice(&fs::read(json_export_path).unwrap()).unwrap();
    assert_eq!(export["session"]["status"], "stopped");
    assert_eq!(export["session"]["segment_count"], 2);
    let segments = export["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0]["chunk_index"], 0);
    assert_eq!(segments[0]["text"], "Meeting segment 00000.");
    assert_eq!(segments[1]["chunk_index"], 2);
    assert_eq!(segments[1]["text"], "Meeting segment 00002.");

    let session_path = Path::new(artifacts["session_dir"].as_str().unwrap()).join("session.json");
    let session: Value = serde_json::from_slice(&fs::read(session_path).unwrap()).unwrap();
    assert_eq!(session["status"], "stopped");
    assert!(session["recorder_pid"].is_null());
    assert!(!runtime
        .data
        .join("meetings")
        .join("active-session")
        .exists());
}

#[test]
fn meet_mic_plus_system_exports_source_labels_for_teams_shape() {
    let runtime = MockRuntime::new();

    let start = runtime.run(
        &[
            "meet",
            "start",
            "--format",
            "json",
            "--source",
            "mic-plus-system",
            "--chunk-seconds",
            "30",
            "--no-llm",
        ],
        2,
        "",
    );
    assert_success(&start);
    let start_json = json_stdout(&start);
    assert_eq!(start_json["source"]["mode"], "mic-plus-system");
    let recorders = start_json["recorders"].as_array().unwrap();
    assert_eq!(recorders.len(), 2);
    let recorder_labels = recorders
        .iter()
        .map(|recorder| recorder["source_label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(recorder_labels.contains(&"user_mic"));
    assert!(recorder_labels.contains(&"system_audio"));
    for recorder in recorders {
        wait_for_chunks(Path::new(recorder["chunks_dir"].as_str().unwrap()), 2);
    }

    let session_id = start_json["session_id"].as_str().unwrap();
    let stop = runtime.run(&["meet", "stop", session_id, "--format", "json"], 2, "");
    assert_success(&stop);
    let stop_json = json_stdout(&stop);

    assert_eq!(stop_json["status"], "stopped");
    assert_eq!(stop_json["source"]["mode"], "mic-plus-system");
    assert_eq!(stop_json["chunks_processed"], 4);
    assert_eq!(stop_json["duration_ms"], 60_000);
    assert_eq!(stop_json["segment_count"], 4);

    let export_path = Path::new(stop_json["artifacts"]["json_export"].as_str().unwrap());
    let export: Value = serde_json::from_slice(&fs::read(export_path).unwrap()).unwrap();
    assert_eq!(export["source"]["mode"], "mic-plus-system");
    assert_eq!(export["retention"]["audio"], false);
    let labels = export["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|segment| segment["source_label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        labels.iter().filter(|label| **label == "user_mic").count(),
        2
    );
    assert_eq!(
        labels
            .iter()
            .filter(|label| **label == "system_audio")
            .count(),
        2
    );
    assert!(export["segments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|segment| segment["chunk_path"].is_null()));

    let segments_jsonl = Path::new(stop_json["artifacts"]["segments_jsonl"].as_str().unwrap());
    let records = fs::read_to_string(segments_jsonl)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records[0]["source"]["mode"], "mic-plus-system");
    assert_eq!(
        records
            .iter()
            .filter(|record| record["record_type"] == "segment")
            .count(),
        4
    );

    assert!(!Path::new(start_json["chunks_dir"].as_str().unwrap()).exists());
}

#[test]
fn meet_start_cleans_up_mic_recorder_when_system_recorder_fails() {
    let runtime = MockRuntime::new();

    let start = runtime.run_with_failed_input(
        &[
            "meet",
            "start",
            "--format",
            "json",
            "--source",
            "mic-plus-system",
            "--chunk-seconds",
            "30",
            "--no-llm",
        ],
        "BlackHole",
    );

    assert!(!start.status.success());
    assert!(String::from_utf8_lossy(&start.stderr).contains("mock ffmpeg failed"));
    assert!(!runtime
        .data
        .join("meetings")
        .join("active-session")
        .exists());
    assert_no_process_contains(&runtime.data.display().to_string());
}

#[test]
fn meet_start_treats_partially_live_multi_stream_session_as_active() {
    let runtime = MockRuntime::new();

    let first = runtime.run(
        &[
            "meet",
            "start",
            "--format",
            "json",
            "--source",
            "mic-plus-system",
            "--chunk-seconds",
            "30",
            "--no-llm",
        ],
        1,
        "",
    );
    assert_success(&first);
    let first_json = json_stdout(&first);
    let recorders = first_json["recorders"].as_array().unwrap();
    for recorder in recorders {
        wait_for_chunks(Path::new(recorder["chunks_dir"].as_str().unwrap()), 1);
    }

    let killed_pid = recorders
        .iter()
        .find(|recorder| recorder["source_label"] == "user_mic")
        .unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    let kill = Command::new("kill")
        .arg("-KILL")
        .arg(killed_pid.to_string())
        .status()
        .unwrap();
    assert!(kill.success());
    wait_until_not_running(killed_pid);

    let second = runtime.run(
        &[
            "meet",
            "start",
            "--format",
            "json",
            "--source",
            "mic-plus-system",
            "--chunk-seconds",
            "30",
            "--no-llm",
        ],
        1,
        "",
    );
    assert!(!second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("meeting session is already recording")
    );

    let session_id = first_json["session_id"].as_str().unwrap();
    let active_id = fs::read_to_string(runtime.data.join("meetings").join("active-session"))
        .unwrap()
        .trim()
        .to_string();
    assert_eq!(active_id, session_id);

    let stop = runtime.run(&["meet", "stop", session_id, "--format", "json"], 1, "");
    assert_success(&stop);
    assert_no_process_contains(&runtime.data.display().to_string());
}

#[test]
fn meet_stop_no_chunks_error_lists_multi_stream_stderr_logs() {
    let runtime = MockRuntime::new();

    let start = runtime.run(
        &[
            "meet",
            "start",
            "--format",
            "json",
            "--source",
            "mic-plus-system",
            "--chunk-seconds",
            "30",
            "--no-llm",
        ],
        0,
        "",
    );
    assert_success(&start);
    let start_json = json_stdout(&start);

    let session_id = start_json["session_id"].as_str().unwrap();
    let stop = runtime.run(&["meet", "stop", session_id, "--format", "json"], 0, "");

    assert!(!stop.status.success());
    let stderr = String::from_utf8_lossy(&stop.stderr);
    assert!(stderr.contains("meeting recording produced no chunk files"));
    assert!(stderr.contains("capture-user_mic.stderr"));
    assert!(stderr.contains("capture-system_audio.stderr"));
    assert!(!stderr.contains("capture.stderr"));
    assert_no_process_contains(&runtime.data.display().to_string());
}

#[test]
fn meet_start_reclaims_active_session_when_recorder_is_gone() {
    let runtime = MockRuntime::new();

    let first = runtime.run(
        &[
            "meet",
            "start",
            "--format",
            "json",
            "--chunk-seconds",
            "30",
            "--no-llm",
        ],
        1,
        "",
    );
    assert_success(&first);
    let first_json = json_stdout(&first);
    wait_for_chunks(Path::new(first_json["chunks_dir"].as_str().unwrap()), 1);

    let first_pid = first_json["recorder_pid"].as_u64().unwrap().to_string();
    let kill = Command::new("kill")
        .arg("-KILL")
        .arg(&first_pid)
        .status()
        .unwrap();
    assert!(kill.success());
    wait_until_not_running(first_pid.parse().unwrap());

    let second = runtime.run(
        &[
            "meet",
            "start",
            "--format",
            "json",
            "--chunk-seconds",
            "30",
            "--no-llm",
        ],
        1,
        "",
    );
    assert_success(&second);
    let second_json = json_stdout(&second);
    assert_ne!(first_json["session_id"], second_json["session_id"]);

    let first_session_path =
        Path::new(first_json["session_dir"].as_str().unwrap()).join("session.json");
    let first_session: Value =
        serde_json::from_slice(&fs::read(first_session_path).unwrap()).unwrap();
    assert_eq!(first_session["status"], "stopped");
    assert!(first_session["recorder_pid"].is_null());

    wait_for_chunks(Path::new(second_json["chunks_dir"].as_str().unwrap()), 1);
    let second_id = second_json["session_id"].as_str().unwrap();
    let stop = runtime.run(&["meet", "stop", second_id, "--format", "json"], 1, "");
    assert_success(&stop);
}

#[test]
fn meet_stop_clears_active_session_when_post_capture_asr_fails() {
    let runtime = MockRuntime::new();

    let start = runtime.run(
        &[
            "meet",
            "start",
            "--format",
            "json",
            "--chunk-seconds",
            "30",
            "--no-llm",
        ],
        2,
        "",
    );
    assert_success(&start);
    let start_json = json_stdout(&start);
    wait_for_chunks(Path::new(start_json["chunks_dir"].as_str().unwrap()), 2);

    let session_id = start_json["session_id"].as_str().unwrap();
    let stop = runtime.run_with_failures(
        &["meet", "stop", session_id, "--format", "json"],
        2,
        "",
        "00000",
    );
    assert!(!stop.status.success());
    assert!(String::from_utf8_lossy(&stop.stderr).contains("mock whisper failed for 00000"));

    let session_path = Path::new(start_json["session_dir"].as_str().unwrap()).join("session.json");
    let session: Value = serde_json::from_slice(&fs::read(session_path).unwrap()).unwrap();
    assert_eq!(session["status"], "stopped");
    assert!(session["recorder_pid"].is_null());
    assert!(!runtime
        .data
        .join("meetings")
        .join("active-session")
        .exists());
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid json stdout: {error}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn wait_for_chunks(chunks_dir: &Path, expected: usize) {
    for _ in 0..100 {
        let count = fs::read_dir(chunks_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry.path().extension().and_then(|value| value.to_str()) == Some("wav")
                    })
                    .count()
            })
            .unwrap_or(0);
        if count == expected {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    panic!(
        "timed out waiting for {expected} chunks in {}",
        chunks_dir.display()
    );
}

fn wait_until_not_running(pid: u32) {
    for _ in 0..100 {
        if !comlink::record::process_is_running(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    panic!("pid {pid} was still running");
}

fn assert_no_process_contains(needle: &str) {
    for _ in 0..100 {
        let output = Command::new("ps")
            .args(["-axo", "command="])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    panic!("process containing {needle} was still running");
}
