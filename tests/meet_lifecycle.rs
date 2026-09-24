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
  sleep 0.1
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

if [ -n "${COMLINK_MOCK_WHISPER_COUNTER:-}" ]; then
  echo invoked >> "$COMLINK_MOCK_WHISPER_COUNTER"
fi
if [ -n "${COMLINK_MOCK_WHISPER_STARTED:-}" ]; then
  touch "$COMLINK_MOCK_WHISPER_STARTED"
fi
if [ -n "${COMLINK_MOCK_WHISPER_BARRIER:-}" ]; then
  for _ in $(seq 1 300); do
    [ -f "$COMLINK_MOCK_WHISPER_BARRIER" ] && break
    sleep 0.1
  done
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

    fn run_env(&self, args: &[&str], chunks: u32, extra_env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_comlink"));
        command
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
            .env("COMLINK_MOCK_DURATION_SECONDS", "30");
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    fn root(&self) -> PathBuf {
        self.home.parent().unwrap().to_path_buf()
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

fn start_meeting(runtime: &MockRuntime, chunks: u32) -> Value {
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
        chunks,
        "",
    );
    assert_success(&start);
    let start_json = json_stdout(&start);
    wait_for_chunks(
        Path::new(start_json["chunks_dir"].as_str().unwrap()),
        chunks as usize,
    );
    start_json
}

fn status_json(runtime: &MockRuntime, id: Option<&str>) -> Value {
    let mut args = vec!["meet", "status"];
    if let Some(id) = id {
        args.push(id);
    }
    args.extend(["--format", "json"]);
    let output = runtime.run(&args, 0, "");
    assert_success(&output);
    json_stdout(&output)
}

/// Poll `meet status <id>` until it reports `want`, dumping diagnostics on
/// timeout instead of hanging.
fn poll_status(runtime: &MockRuntime, id: &str, want: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..300 {
        last = status_json(runtime, Some(id));
        if last["status"] == want {
            return last;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let log = runtime.data.join("meetings").join(id).join("finalize.log");
    panic!(
        "timed out waiting for status {want}; last status: {last:#}\nfinalize.log:\n{}",
        fs::read_to_string(log).unwrap_or_default()
    );
}

fn wait_for_file(path: &Path) {
    for _ in 0..300 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {}", path.display());
}

fn invocation_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

fn store_for(runtime: &MockRuntime) -> comlink::meet::FileMeetingStore {
    comlink::meet::FileMeetingStore::new(&comlink::config::ConfigPaths {
        home_dir: runtime.home.clone(),
        config_file: runtime.home.join("config.json"),
        data_dir: runtime.data.clone(),
        database_file: runtime.data.join("history.sqlite3"),
        audio_dir: runtime.data.join("audio"),
    })
}

fn assert_exports_match_golden(runtime: &MockRuntime, id: &str) {
    let root = runtime.root();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/meet");
    for (format, fixture) in [("json", "export.json"), ("md", "export.md")] {
        let output = runtime.run(&["meet", "export", id, "--format", format], 0, "");
        assert_success(&output);
        let actual = normalize_golden(&String::from_utf8_lossy(&output.stdout), &root, id);
        let expected = fs::read_to_string(fixtures.join(fixture)).unwrap();
        assert_eq!(
            actual, expected,
            "detached export differs from sync golden {fixture}"
        );
    }
}

fn session_state(runtime: &MockRuntime, id: &str) -> Value {
    let path = runtime.data.join("meetings").join(id).join("session.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn meet_status_reports_none_active_and_stale_recorder() {
    let runtime = MockRuntime::new();

    let none = status_json(&runtime, None);
    assert_eq!(none["schema_version"], "comlink.meeting.v1");
    assert_eq!(none["status"], "none");
    assert!(none["session_id"].is_null());
    assert_eq!(none["stale"], false);

    let unknown = runtime.run(&["meet", "status", "missing", "--format", "json"], 0, "");
    assert_eq!(unknown.status.code(), Some(1));
    assert!(unknown.stdout.is_empty());

    let start_json = start_meeting(&runtime, 2);
    let id = start_json["session_id"].as_str().unwrap();
    let active = status_json(&runtime, None);
    assert_eq!(active["session_id"], id);
    assert_eq!(active["status"], "recording");
    assert_eq!(active["stale"], false);
    assert!(active["stale_reason"].is_null());
    assert_eq!(active["chunk_count"], 2);
    assert!(active["finalizer"].is_null());
    // Mock chunks are not PCM, so there is nothing measurable yet.
    assert!(active["audio_level"].is_null());
    let recorders = active["recorders"].as_array().unwrap();
    assert_eq!(recorders.len(), 1);
    assert_eq!(recorders[0]["alive"], true);
    assert_eq!(recorders[0]["pid"], start_json["recorder_pid"]);
    for key in [
        "elapsed_ms",
        "warnings",
        "stale",
        "stale_reason",
        "audio_level",
        "finalizer",
    ] {
        assert!(active.get(key).is_some(), "missing status key {key}");
    }

    let pid = start_json["recorder_pid"].as_u64().unwrap() as u32;
    assert!(Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status()
        .unwrap()
        .success());
    wait_until_not_running(pid);

    let started = std::time::Instant::now();
    let stale = status_json(&runtime, None);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(stale["status"], "recording");
    assert_eq!(stale["stale"], true);
    assert_eq!(stale["recorders"][0]["alive"], false);
    assert!(stale["stale_reason"]
        .as_str()
        .unwrap()
        .contains("no recorder process is running"));

    let text = runtime.run(&["meet", "status"], 0, "");
    assert_success(&text);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(&format!("session_id: {id}")));
    assert!(text.contains("stale: true"));
}

#[test]
fn meet_stop_detach_returns_immediately_and_finalizes_in_background() {
    let runtime = MockRuntime::new();
    let barrier = runtime.root().join("whisper-barrier");
    let started_flag = runtime.root().join("whisper-started");
    let counter = runtime.root().join("whisper-count");
    let barrier_env = [
        ("COMLINK_MOCK_WHISPER_BARRIER", barrier.to_str().unwrap()),
        (
            "COMLINK_MOCK_WHISPER_STARTED",
            started_flag.to_str().unwrap(),
        ),
        ("COMLINK_MOCK_WHISPER_COUNTER", counter.to_str().unwrap()),
    ];

    let start_json = start_meeting(&runtime, 2);
    let id = start_json["session_id"].as_str().unwrap();

    let clock = std::time::Instant::now();
    let stop = runtime.run_env(
        &["meet", "stop", "--detach", "--format", "json"],
        2,
        &barrier_env,
    );
    let elapsed = clock.elapsed();
    assert_success(&stop);
    assert!(
        elapsed < Duration::from_secs(1),
        "stop --detach took {elapsed:?}"
    );
    let stop_json = json_stdout(&stop);
    assert_eq!(stop_json["status"], "transcribing");
    assert_eq!(stop_json["session_id"], id);
    assert_eq!(stop_json["chunk_count"], 2);
    assert!(stop_json["finalizer_pid"].as_u64().unwrap() > 0);
    assert!(!runtime
        .data
        .join("meetings")
        .join("active-session")
        .exists());

    wait_for_file(&started_flag);
    let transcribing = status_json(&runtime, None);
    assert_eq!(transcribing["session_id"], id);
    assert_eq!(transcribing["status"], "transcribing");
    assert_eq!(transcribing["finalizer"]["alive"], true);
    assert_eq!(transcribing["stale"], false);

    fs::write(&barrier, "go").unwrap();
    let stopped = poll_status(&runtime, id, "stopped");
    assert_eq!(stopped["stale"], false);
    assert_eq!(stopped["finalizer"], Value::Null);
    assert_eq!(invocation_count(&counter), 2);

    let state = session_state(&runtime, id);
    assert_eq!(state["status"], "stopped");
    assert!(state.get("finalizer").is_none());
    assert!(state.get("error").is_none());
    assert_eq!(state["chunks_processed"], 2);
    assert_exports_match_golden(&runtime, id);

    // With nothing active or transcribing, bare status is `none` again.
    assert_eq!(status_json(&runtime, None)["status"], "none");

    // Repeated finalize is an idempotent no-op: same status, no new ASR.
    let first = runtime.run_env(
        &["meet", "finalize", id, "--format", "json"],
        0,
        &barrier_env,
    );
    assert_success(&first);
    let second = runtime.run_env(
        &["meet", "finalize", id, "--format", "json"],
        0,
        &barrier_env,
    );
    assert_success(&second);
    let root = runtime.root();
    let first_text = normalize_golden(&String::from_utf8_lossy(&first.stdout), &root, id);
    let second_text = normalize_golden(&String::from_utf8_lossy(&second.stdout), &root, id);
    assert_eq!(first_text, second_text);
    let golden_stop = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/meet/stop.json"),
    )
    .unwrap();
    assert_eq!(
        first_text, golden_stop,
        "finalize must print the sync stop shape"
    );
    assert_eq!(invocation_count(&counter), 2, "finalize re-ran whisper");
}

#[test]
fn meet_finalize_marks_failed_on_whisper_error_and_recovers_on_rerun() {
    let runtime = MockRuntime::new();
    let start_json = start_meeting(&runtime, 2);
    let id = start_json["session_id"].as_str().unwrap();

    let stop = runtime.run_env(
        &["meet", "stop", id, "--detach", "--format", "json"],
        2,
        &[("COMLINK_MOCK_FAIL_CHUNKS", "00001")],
    );
    assert_success(&stop);
    let failed = poll_status(&runtime, id, "failed");
    let error = failed["error"].as_str().unwrap();
    assert!(error.contains("mock whisper failed for 00001"), "{error}");
    assert_eq!(failed["stale"], false);
    let log =
        fs::read_to_string(runtime.data.join("meetings").join(id).join("finalize.log")).unwrap();
    assert!(log.contains("mock whisper failed"));
    assert!(
        !log.contains("Meeting segment"),
        "finalize.log leaked transcript text"
    );

    // Export is refused while failed.
    let export = runtime.run(&["meet", "export", id, "--format", "json"], 0, "");
    assert_eq!(export.status.code(), Some(1));

    // Rerun with whisper healthy: finalize recovers to stopped.
    let rerun = runtime.run(&["meet", "finalize", id, "--format", "json"], 0, "");
    assert_success(&rerun);
    assert_eq!(json_stdout(&rerun)["status"], "stopped");
    let state = session_state(&runtime, id);
    assert_eq!(state["status"], "stopped");
    assert!(state.get("error").is_none());
    assert!(state.get("finalizer").is_none());
    assert_exports_match_golden(&runtime, id);

    // A forced whisper failure surfaces the ASR exit code (3) from finalize.
    let second = start_meeting(&runtime, 1);
    let second_id = second["session_id"].as_str().unwrap();
    let barrier = runtime.root().join("never");
    let stop = runtime.run_env(
        &["meet", "stop", second_id, "--detach", "--format", "json"],
        1,
        &[
            ("COMLINK_MOCK_FAIL_CHUNKS", "00000"),
            ("COMLINK_MOCK_WHISPER_BARRIER", barrier.to_str().unwrap()),
        ],
    );
    assert_success(&stop);
    let finalizer_pid = json_stdout(&stop)["finalizer_pid"].as_u64().unwrap();
    // Kill the background finalizer so this foreground finalize owns the run.
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(format!("-{finalizer_pid}"))
        .status();
    wait_until_not_running(finalizer_pid as u32);
    let direct = runtime.run_env(
        &["meet", "finalize", second_id, "--format", "json"],
        0,
        &[("COMLINK_MOCK_FAIL_CHUNKS", "00000")],
    );
    assert_eq!(direct.status.code(), Some(3));
    assert!(direct.stdout.is_empty());
    assert_eq!(session_state(&runtime, second_id)["status"], "failed");
}

#[test]
fn meet_finalize_recovers_after_finalizer_is_killed_mid_transcription() {
    let runtime = MockRuntime::new();
    let barrier = runtime.root().join("whisper-barrier");
    let started_flag = runtime.root().join("whisper-started");
    let start_json = start_meeting(&runtime, 2);
    let id = start_json["session_id"].as_str().unwrap();

    let stop = runtime.run_env(
        &["meet", "stop", id, "--detach", "--format", "json"],
        2,
        &[
            ("COMLINK_MOCK_WHISPER_BARRIER", barrier.to_str().unwrap()),
            (
                "COMLINK_MOCK_WHISPER_STARTED",
                started_flag.to_str().unwrap(),
            ),
        ],
    );
    assert_success(&stop);
    let finalizer_pid = json_stdout(&stop)["finalizer_pid"].as_u64().unwrap() as u32;
    wait_for_file(&started_flag);
    assert!(store_for(&runtime).is_session_locked(id).unwrap());

    // Real crash: SIGKILL only the finalizer (its whisper child is orphaned).
    assert!(Command::new("kill")
        .arg("-KILL")
        .arg(finalizer_pid.to_string())
        .status()
        .unwrap()
        .success());
    wait_until_not_running(finalizer_pid);

    let stale = status_json(&runtime, Some(id));
    assert_eq!(stale["status"], "transcribing");
    assert_eq!(stale["stale"], true);
    assert_eq!(stale["finalizer"]["alive"], false);
    assert!(stale["stale_reason"]
        .as_str()
        .unwrap()
        .contains(&format!("comlink meet finalize {id}")));
    // The kernel released the dead holder's lock.
    let lock = store_for(&runtime)
        .lock_session(id, comlink::meet::LockWait::Try, "test")
        .unwrap();
    drop(lock);

    fs::write(&barrier, "go").unwrap();
    let rerun = runtime.run(&["meet", "finalize", id, "--format", "json"], 0, "");
    assert_success(&rerun);
    assert_eq!(json_stdout(&rerun)["status"], "stopped");
    assert_exports_match_golden(&runtime, id);
}

#[test]
fn meet_plain_stop_stays_synchronous() {
    let runtime = MockRuntime::new();
    let start_json = start_meeting(&runtime, 2);
    let id = start_json["session_id"].as_str().unwrap();
    let stop = runtime.run(&["meet", "stop", "--format", "json"], 2, "");
    assert_success(&stop);
    // The transcript exists by the time the synchronous stop returns.
    assert_eq!(json_stdout(&stop)["status"], "stopped");
    assert_eq!(session_state(&runtime, id)["status"], "stopped");
    assert!(!runtime
        .data
        .join("meetings")
        .join(id)
        .join("finalize.log")
        .exists());
    assert_exports_match_golden(&runtime, id);
}

#[test]
fn meet_detach_and_finalize_text_output_and_status_finalize_log() {
    let runtime = MockRuntime::new();
    let start_json = start_meeting(&runtime, 2);
    let id = start_json["session_id"].as_str().unwrap();
    let log = runtime.data.join("meetings").join(id).join("finalize.log");

    // Default --format is text.
    let stop = runtime.run(&["meet", "stop", id, "--detach"], 2, "");
    assert_success(&stop);
    let text = String::from_utf8_lossy(&stop.stdout);
    for needle in [
        format!("session_id: {id}\n"),
        "status: transcribing\n".to_string(),
        "chunk_count: 2\n".to_string(),
        "finalizer_pid: ".to_string(),
        format!("finalize_log: {}\n", log.display()),
        "markdown_export: ".to_string(),
    ] {
        assert!(text.contains(&needle), "missing {needle:?} in:\n{text}");
    }
    assert!(serde_json::from_slice::<Value>(&stop.stdout).is_err());

    let stopped = poll_status(&runtime, id, "stopped");
    assert_eq!(stopped["finalize_log"], log.display().to_string());
    let status_text = runtime.run(&["meet", "status", id], 0, "");
    assert_success(&status_text);
    assert!(String::from_utf8_lossy(&status_text.stdout)
        .contains(&format!("finalize_log: {}\n", log.display())));

    let finalize = runtime.run(&["meet", "finalize", id], 0, "");
    assert_success(&finalize);
    let text = String::from_utf8_lossy(&finalize.stdout);
    for needle in [
        format!("session_id: {id}\n"),
        "status: stopped\n".to_string(),
        "chunks_processed: 2\n".to_string(),
        "segment_count: 2\n".to_string(),
        "json_export: ".to_string(),
        "retention: metadata=true, transcripts=true, audio=false\n".to_string(),
    ] {
        assert!(text.contains(&needle), "missing {needle:?} in:\n{text}");
    }

    // A synchronously stopped session has no finalize log: null.
    let second = start_meeting(&runtime, 2);
    let second_id = second["session_id"].as_str().unwrap();
    assert_success(&runtime.run(&["meet", "stop", second_id, "--format", "json"], 2, ""));
    assert!(status_json(&runtime, Some(second_id))["finalize_log"].is_null());
}

#[test]
fn privacy_audit_flags_leftover_meeting_audio_until_finalize_cleans_it() {
    let runtime = MockRuntime::new();
    let audit = |runtime: &MockRuntime| {
        let output = runtime.run(&["privacy", "audit", "--format", "json"], 0, "");
        assert_success(&output);
        json_stdout(&output)
    };
    assert_eq!(audit(&runtime)["meeting_audio"]["clean"], true);

    let start_json = start_meeting(&runtime, 2);
    let id = start_json["session_id"].as_str().unwrap();
    assert_success(&runtime.run(&["meet", "stop", id, "--format", "json"], 2, ""));
    let chunks_dir = PathBuf::from(start_json["chunks_dir"].as_str().unwrap());
    assert!(!chunks_dir.exists(), "retention off deletes chunks on stop");
    let clean = audit(&runtime);
    assert_eq!(clean["meeting_audio"]["clean"], true);
    assert_eq!(clean["retention"]["audio"], false);

    // Chunks left behind by the old stopped-before-cleanup ordering.
    fs::create_dir_all(&chunks_dir).unwrap();
    fs::write(chunks_dir.join("chunk-00000.wav"), "leftover").unwrap();
    let dirty = audit(&runtime);
    assert_eq!(dirty["meeting_audio"]["clean"], false);
    let leftovers = dirty["meeting_audio"]["unretained_leftovers"]
        .as_array()
        .unwrap();
    assert_eq!(leftovers.len(), 1);
    assert_eq!(leftovers[0]["session_id"], id);
    assert_eq!(leftovers[0]["status"], "stopped");
    assert_eq!(leftovers[0]["chunk_files"], 1);
    let text = runtime.run(&["privacy", "audit"], 0, "");
    assert_success(&text);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("meeting_audio: clean=false unretained_leftovers=1"),
        "{text}"
    );
    assert!(
        text.contains(&format!("comlink meet finalize {id}")),
        "{text}"
    );

    let finalize = runtime.run(&["meet", "finalize", id, "--format", "json"], 0, "");
    assert_success(&finalize);
    assert!(!chunks_dir.exists(), "finalize deleted the leftovers");
    assert_eq!(session_state(&runtime, id)["status"], "stopped");
    assert_eq!(audit(&runtime)["meeting_audio"]["clean"], true);
}

#[test]
fn privacy_audit_survives_unreadable_meeting_dirs_and_names_them() {
    let runtime = MockRuntime::new();
    let start_json = start_meeting(&runtime, 2);
    let id = start_json["session_id"].as_str().unwrap();
    assert_success(&runtime.run(&["meet", "stop", id, "--format", "json"], 2, ""));
    let chunks_dir = PathBuf::from(start_json["chunks_dir"].as_str().unwrap());
    let meetings_root = chunks_dir.parent().unwrap().parent().unwrap().to_path_buf();

    // A corrupt session.json whose chunk WAVs remain.
    let corrupt = meetings_root.join("corrupt-session");
    fs::create_dir_all(corrupt.join("chunks")).unwrap();
    fs::write(corrupt.join("session.json"), "{").unwrap();
    fs::write(corrupt.join("chunks/chunk-00000.wav"), "leftover").unwrap();
    // An unreadable chunks dir on the stopped session.
    fs::create_dir_all(&chunks_dir).unwrap();
    fs::set_permissions(&chunks_dir, fs::Permissions::from_mode(0o000)).unwrap();
    let output = runtime.run(&["privacy", "audit", "--format", "json"], 0, "");
    let text = runtime.run(&["privacy", "audit"], 0, "");
    fs::set_permissions(&chunks_dir, fs::Permissions::from_mode(0o755)).unwrap();

    assert_success(&output);
    let audit = json_stdout(&output);
    assert_eq!(audit["meeting_audio"]["clean"], false);
    let leftovers = audit["meeting_audio"]["unretained_leftovers"]
        .as_array()
        .unwrap();
    assert_eq!(leftovers.len(), 2, "{leftovers:?}");
    let unreadable = leftovers
        .iter()
        .find(|entry| entry["session_id"] == id)
        .unwrap();
    let chunks_text = chunks_dir.display().to_string();
    assert!(
        unreadable["reason"]
            .as_str()
            .unwrap()
            .contains(&format!("unreadable chunks dir under {chunks_text}")),
        "{unreadable}"
    );
    let corrupt_entry = leftovers
        .iter()
        .find(|entry| entry["session_id"] == "corrupt-session")
        .unwrap();
    assert_eq!(corrupt_entry["status"], "unknown");
    assert_eq!(corrupt_entry["chunk_files"], 1);
    assert_eq!(corrupt_entry["session_dir"], corrupt.display().to_string());
    assert!(corrupt_entry["reason"]
        .as_str()
        .unwrap()
        .contains("unreadable session.json"));

    assert_success(&text);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("meeting_audio: clean=false unretained_leftovers=2 scan_errors=0"),
        "{text}"
    );
    assert!(text.contains(&chunks_text), "{text}");
}

/// Golden regression for the byte-level CLI contract of `meet start`, `meet
/// stop`, and `meet export`. The fixtures under `tests/fixtures/meet/` were
/// captured from the pre-refactor revision (97988d2) with:
///
/// ```bash
/// COMLINK_UPDATE_GOLDENS=1 cargo test --test meet_lifecycle meet_cli_output_matches_golden_fixtures
/// ```
///
/// Only nondeterministic values (temp root, session id, pids, times) are
/// normalized; key order, nullability, and types are compared as raw text.
#[test]
fn meet_cli_output_matches_golden_fixtures() {
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
    let session_id = start_json["session_id"].as_str().unwrap().to_string();

    let stop = runtime.run(&["meet", "stop", &session_id, "--format", "json"], 2, "");
    assert_success(&stop);
    let export_json = runtime.run(&["meet", "export", &session_id, "--format", "json"], 2, "");
    assert_success(&export_json);
    let export_md = runtime.run(&["meet", "export", &session_id, "--format", "md"], 2, "");
    assert_success(&export_md);

    let root = runtime.root();
    let cases = [
        ("start.json", &start.stdout),
        ("stop.json", &stop.stdout),
        ("export.json", &export_json.stdout),
        ("export.md", &export_md.stdout),
    ];
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/meet");
    for (name, stdout) in cases {
        let actual = normalize_golden(&String::from_utf8_lossy(stdout), &root, &session_id);
        let fixture = fixtures.join(name);
        if std::env::var_os("COMLINK_UPDATE_GOLDENS").is_some() {
            fs::create_dir_all(&fixtures).unwrap();
            fs::write(&fixture, &actual).unwrap();
            continue;
        }
        let expected = fs::read_to_string(&fixture)
            .unwrap_or_else(|error| panic!("missing golden {}: {error}", fixture.display()));
        assert_eq!(
            actual, expected,
            "golden mismatch for {name}; rerun with COMLINK_UPDATE_GOLDENS=1 only if the contract change is intended"
        );
    }
}

const GOLDEN_VOLATILE_JSON_KEYS: &[&str] = &[
    "elapsed_ms",
    "recorder_pid",
    "pid",
    "started_at_ms",
    "stopped_at_ms",
    "process_started_at",
];

fn normalize_golden(text: &str, root: &Path, session_id: &str) -> String {
    let text = text
        .replace(&root.display().to_string(), "<ROOT>")
        .replace(session_id, "<SESSION>");
    let mut normalized = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        normalized.push_str(&normalize_golden_line(line));
    }
    normalized
}

fn normalize_golden_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    for key in GOLDEN_VOLATILE_JSON_KEYS {
        let prefix = format!("\"{key}\": ");
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            if rest.trim_end().trim_end_matches(',') == "null" {
                return line.to_string();
            }
            let comma = if rest.trim_end().ends_with(',') {
                ","
            } else {
                ""
            };
            let newline = if line.ends_with('\n') { "\n" } else { "" };
            return format!("{indent}{prefix}\"<V>\"{comma}{newline}");
        }
    }
    for label in ["- **Started:** ", "- **Stopped:** "] {
        if trimmed.starts_with(label) {
            let newline = if line.ends_with('\n') { "\n" } else { "" };
            return format!("{indent}{label}<V>{newline}");
        }
    }
    line.to_string()
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
    // Generous bound: mock recorders are bash scripts and can start slowly
    // when the machine is loaded; the loop exits as soon as chunks appear.
    for _ in 0..500 {
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
    for _ in 0..500 {
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
