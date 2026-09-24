#![cfg(unix)]
//! The real `comlink` binary: a session started by the CLI is visible and
//! stoppable through a spawned `comlink mcp`, and the reverse. Also checks
//! that `comlink mcp` writes only JSON-RPC frames to stdout and nothing to
//! stderr, and that `config set mcp.allow_start` takes effect without a
//! server restart.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use serde_json::{json, Value};

const FFMPEG: &str = r#"#!/usr/bin/env bash
set -euo pipefail
out="${@: -1}"
mkdir -p "$(dirname "$out")"
for index in 0 1; do
  printf 'mock wav %s\n' "$index" > "$(printf "$out" "$index")"
done
trap 'exit 0' INT TERM
while true; do
  sleep 0.1
done
"#;

const WHISPER: &str = r#"#!/usr/bin/env bash
set -euo pipefail
out=""
wav=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -of) shift; out="$1" ;;
    -f) shift; wav="$1" ;;
  esac
  shift || true
done
name="$(basename "$wav" .wav)"
printf 'Interop segment %s.\n' "${name#chunk-}" > "$out.txt"
"#;

struct Env {
    _tempdir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let root = tempdir.path().to_path_buf();
        write_executable(&root.join("mock-ffmpeg"), FFMPEG);
        write_executable(
            &root.join("mock-ffprobe"),
            "#!/usr/bin/env bash\nprintf '30\\n'\n",
        );
        write_executable(&root.join("mock-whisper"), WHISPER);
        fs::write(root.join("model.bin"), "mock model\n").unwrap();
        Self {
            _tempdir: tempdir,
            root,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_comlink"));
        command
            .env("COMLINK_HOME", self.root.join("home"))
            .env("COMLINK_DATA_DIR", self.root.join("data"))
            .env("COMLINK_FFMPEG", self.root.join("mock-ffmpeg"))
            .env("COMLINK_FFPROBE", self.root.join("mock-ffprobe"))
            .env("COMLINK_WHISPER_CPP", self.root.join("mock-whisper"))
            .env("COMLINK_WHISPER_MODEL", self.root.join("model.bin"))
            .env("COMLINK_LLM_ENABLED", "false")
            .env("COMLINK_RECORD_DEVICE", ":0")
            .env_remove("COMLINK_MCP_ALLOW_START");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "comlink {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn cli_json(&self, args: &[&str]) -> Value {
        serde_json::from_slice(&self.run(args).stdout).unwrap()
    }

    fn poll_cli_status(&self, id: &str, want: &str) -> Value {
        let mut last = Value::Null;
        for _ in 0..600 {
            last = self.cli_json(&["meet", "status", id, "--format", "json"]);
            if last["status"] == want {
                return last;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let log = self
            .root
            .join("data/meetings")
            .join(id)
            .join("finalize.log");
        panic!(
            "timed out waiting for {want}: {last:#}\n{}",
            fs::read_to_string(log).unwrap_or_default()
        );
    }

    fn mcp(&self) -> McpProcess {
        let mut child = self
            .command()
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || read_lines(stdout, sender));
        let mut process = McpProcess {
            child,
            stdin: Some(stdin),
            lines: receiver,
            reader: Some(reader),
            next_id: 1,
            raw: Vec::new(),
        };
        let init = process.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "interop", "version": "0"}
            }),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        process.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        process
    }
}

fn read_lines(stdout: ChildStdout, sender: mpsc::Sender<String>) {
    for line in BufReader::new(stdout).lines() {
        match line {
            Ok(line) => {
                if sender.send(line).is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    reader: Option<thread::JoinHandle<()>>,
    next_id: u64,
    raw: Vec<String>,
}

impl McpProcess {
    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(60))
                .expect("comlink mcp response timed out");
            self.raw.push(line.clone());
            let message: Value = serde_json::from_str(&line).expect("stdout line is JSON");
            if message["id"] == id {
                return message;
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        response["result"].clone()
    }

    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], false, "{name}: {result:#}");
        result["structuredContent"].clone()
    }

    /// Close stdin, wait for exit, and check stdout was only JSON-RPC and
    /// stderr was empty.
    fn finish(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        self.reader.take().unwrap().join().unwrap();
        self.raw.extend(self.lines.try_iter());
        let mut stderr = String::new();
        std::io::Read::read_to_string(self.child.stderr.as_mut().unwrap(), &mut stderr).unwrap();
        assert!(
            status.success(),
            "comlink mcp exited with {status}: {stderr}"
        );
        assert_eq!(stderr, "", "comlink mcp wrote to stderr");
        assert!(!self.raw.is_empty());
        for line in &self.raw {
            let message: Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("non-JSON stdout {line:?}: {error}"));
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
        }
    }
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn wait_for_chunks(dir: &str) {
    for _ in 0..500 {
        let count = fs::read_dir(dir)
            .map(|entries| entries.filter_map(Result::ok).count())
            .unwrap_or(0);
        if count >= 2 {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("no chunks in {dir}");
}

#[test]
fn cli_started_meeting_is_visible_and_stoppable_through_mcp() {
    let env = Env::new();
    let start = env.cli_json(&[
        "meet",
        "start",
        "--format",
        "json",
        "--chunk-seconds",
        "30",
        "--no-llm",
    ]);
    let id = start["session_id"].as_str().unwrap().to_string();
    wait_for_chunks(start["chunks_dir"].as_str().unwrap());

    let mut mcp = env.mcp();
    let status = mcp.ok("meeting_status", json!({}));
    assert_eq!(status["session_id"], id);
    assert_eq!(status["status"], "recording");
    let list = mcp.ok("meeting_list", json!({}));
    assert_eq!(list["sessions"][0]["session_id"], id);

    let stop = mcp.ok("meeting_stop", json!({}));
    assert_eq!(stop["status"], "transcribing");
    assert_eq!(stop["session_id"], id);

    // The detached finalizer is the real `comlink meet finalize`, launched
    // by the MCP server; the CLI sees it finish.
    env.poll_cli_status(&id, "stopped");
    let export = env.run(&["meet", "export", &id, "--format", "md"]);
    assert!(String::from_utf8_lossy(&export.stdout).contains("Interop segment 00000"));
    let transcript = mcp.ok(
        "meeting_get_transcript",
        json!({"id": id, "format": "json"}),
    );
    assert_eq!(transcript["content"]["session"]["session_id"], id);
    mcp.finish();
}

#[test]
fn mcp_started_meeting_is_stoppable_by_the_cli_and_allow_start_needs_no_restart() {
    let env = Env::new();
    let mut mcp = env.mcp();
    let start_args = json!({"source": "mic-only", "mode": "raw", "no_llm": true});

    let refused = mcp.call("meeting_start", start_args.clone());
    assert_eq!(refused["isError"], true);
    assert_eq!(
        refused["structuredContent"]["error_code"],
        "mcp_start_disabled"
    );

    // Opt in from the CLI while the server keeps running.
    let set = env.run(&["config", "set", "mcp.allow_start", "true"]);
    assert!(String::from_utf8_lossy(&set.stdout).contains("mcp.allow_start=true"));
    assert!(set.stderr.is_empty());

    let start = mcp.ok("meeting_start", start_args);
    let id = start["session_id"].as_str().unwrap().to_string();
    wait_for_chunks(start["chunks_dir"].as_str().unwrap());

    let cli_status = env.cli_json(&["meet", "status", "--format", "json"]);
    assert_eq!(cli_status["session_id"], id);
    assert_eq!(cli_status["status"], "recording");
    let stop = env.cli_json(&["meet", "stop", "--format", "json"]);
    assert_eq!(stop["status"], "stopped");
    assert_eq!(stop["session_id"], id);

    let status = mcp.ok("meeting_status", json!({"id": id}));
    assert_eq!(status["status"], "stopped");
    let transcript = mcp.ok("meeting_get_transcript", json!({"format": "md"}));
    assert!(transcript["content"]
        .as_str()
        .unwrap()
        .contains("Interop segment 00001"));
    // The recorder the MCP server spawned was stopped by the CLI and reaped
    // by the still-running server.
    let recorder = start["recorder_pid"].as_u64().unwrap().to_string();
    let mut reaped = false;
    for _ in 0..200 {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &recorder])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&output.stdout).trim().is_empty() {
            reaped = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(reaped, "recorder {recorder} left behind by the MCP server");
    mcp.finish();
}

fn failed(output: &Output, code: i32) -> String {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn invalid_mode_errors_and_exit_codes_are_unchanged_for_transcribe_record_and_meet_start() {
    let env = Env::new();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/short.wav");
    let fixture = fixture.to_str().unwrap();
    for args in [
        vec![
            "transcribe",
            fixture,
            "--mode",
            "no-such-mode",
            "--format",
            "json",
        ],
        vec!["record", "--mode", "no-such-mode", "--format", "json"],
        vec![
            "meet",
            "start",
            "--mode",
            "no-such-mode",
            "--format",
            "json",
        ],
    ] {
        let output = env.command().args(&args).output().unwrap();
        let stderr = failed(&output, 1);
        assert_eq!(
            stderr, "error: text mode not found: no-such-mode\n",
            "{args:?}"
        );
    }
    // Mode is still checked before the model, and a missing model is exit 3.
    let output = env
        .command()
        .env_remove("COMLINK_WHISPER_MODEL")
        .args(["meet", "start", "--mode", "no-such-mode"])
        .output()
        .unwrap();
    assert!(failed(&output, 1).contains("text mode not found"));
    let output = env
        .command()
        .env_remove("COMLINK_WHISPER_MODEL")
        .args(["meet", "start", "--mode", "raw"])
        .output()
        .unwrap();
    assert!(failed(&output, 3).contains("model path is required"));
    assert!(!env.root.join("data/meetings/active-session").exists());
}

#[test]
fn config_set_and_privacy_audit_report_mcp_state() {
    let env = Env::new();
    let output = env
        .command()
        .args(["config", "set", "retention.audio", "true"])
        .output()
        .unwrap();
    assert!(failed(&output, 1).contains("unknown config key: retention.audio"));
    let output = env
        .command()
        .args(["config", "set", "mcp.allow_start", "perhaps"])
        .output()
        .unwrap();
    failed(&output, 1);

    let audit = env.cli_json(&["privacy", "audit", "--format", "json"]);
    let keys: Vec<&String> = audit.as_object().unwrap().keys().collect();
    assert!(keys.contains(&&"meeting_audio".to_string()));
    assert_eq!(audit["mcp"]["transport"], "stdio");
    assert_eq!(audit["mcp"]["network_listener"], false);
    assert_eq!(audit["mcp"]["allow_start"], false);
    assert_eq!(audit["mcp"]["transcripts_sent_to_calling_model"], true);
    // meeting_audio keeps its shape.
    assert!(audit["meeting_audio"]["clean"].is_boolean());

    env.run(&["config", "set", "mcp.allow_start", "true"]);
    let audit = env.cli_json(&["privacy", "audit", "--format", "json"]);
    assert_eq!(audit["mcp"]["allow_start"], true);
    let text = String::from_utf8_lossy(&env.run(&["privacy", "audit"]).stdout).to_string();
    assert!(text.contains("mcp: transport=stdio network_listener=false allow_start=true"));
    let shown = String::from_utf8_lossy(&env.run(&["config", "show"]).stdout).to_string();
    assert!(shown.contains("mcp: allow_start=true"));

    // The env var overrides the file, and `config set` says so.
    let output = env
        .command()
        .env("COMLINK_MCP_ALLOW_START", "false")
        .args(["config", "set", "mcp.allow_start", "true"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("COMLINK_MCP_ALLOW_START=false"));
}
