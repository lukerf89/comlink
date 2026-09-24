//! In-process harness for the meeting service: a mock runtime whose scripts
//! have their behaviour baked in (no environment mutation), plus launcher
//! adapters for exercising detached finalize without spawning a process.

#![allow(dead_code)]

use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use comlink::{
    config::{Config, ConfigPaths, ResolvedConfig},
    deps::RuntimeDeps,
    error::ComlinkError,
    mcp::{ComlinkMcp, McpContextFactory},
    meet::{self, MeetingSessionState},
    meet_service::{self, FinalizeLauncher, MeetContext, MeetStopStatus},
    record::{self, ProcessIdentity},
};
use serde_json::{json, Value};

pub struct ServiceHarness {
    _tempdir: tempfile::TempDir,
    pub root: PathBuf,
    pub runtime: RuntimeDeps,
    pub resolved: ResolvedConfig,
    pub whisper_counter: PathBuf,
}

pub struct MockOptions {
    pub chunks: u32,
    pub fail_chunks: &'static str,
    pub retain_audio: bool,
    /// Write the real `tests/fixtures/audio/silence.wav` as every chunk, so
    /// the near-silent capture warning fires (whisper still returns text).
    pub silent: bool,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            chunks: 2,
            fail_chunks: "",
            retain_audio: false,
            silent: false,
        }
    }
}

pub fn silence_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/silence.wav")
}

impl ServiceHarness {
    pub fn new(options: MockOptions) -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let root = tempdir.path().to_path_buf();
        let ffmpeg = root.join("mock-ffmpeg");
        let ffprobe = root.join("mock-ffprobe");
        let whisper = root.join("mock-whisper");
        let model = root.join("model.bin");
        let whisper_counter = root.join("whisper-count");

        write_executable(
            &ffmpeg,
            &format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
case " $* " in
  *" -list_devices true "*)
    {{
      echo "[AVFoundation indev @ 0x1] AVFoundation audio devices:"
      echo "[AVFoundation indev @ 0x1] [0] MacBook Pro Microphone"
      echo "[AVFoundation indev @ 0x1] [1] BlackHole 2ch"
    }} >&2
    exit 1
    ;;
esac
out="${{@: -1}}"
chunks={chunks}
mkdir -p "$(dirname "$out")"
if [ "$chunks" -gt 0 ]; then
  for index in $(seq 0 $((chunks - 1))); do
    chunk="$(printf "$out" "$index")"
    if [ -n "{silent}" ]; then
      cp "{silent}" "$chunk"
    else
      printf 'mock wav %s\n' "$index" > "$chunk"
    fi
  done
fi
trap 'exit 0' INT TERM
while true; do
  sleep 0.1
done
"#,
                chunks = options.chunks,
                silent = if options.silent {
                    silence_fixture().display().to_string()
                } else {
                    String::new()
                }
            ),
        );
        write_executable(&ffprobe, "#!/usr/bin/env bash\nprintf '30\\n'\n");
        write_executable(
            &whisper,
            &whisper_script(&whisper_counter, options.fail_chunks),
        );
        fs::write(&model, "mock model\n").unwrap();

        let data = root.join("data");
        let home = root.join("home");
        let mut config = Config::default();
        config.retention.audio = options.retain_audio;
        let resolved = ResolvedConfig {
            paths: ConfigPaths {
                config_file: home.join("config.json"),
                home_dir: home,
                database_file: data.join("history.sqlite3"),
                audio_dir: data.join("audio"),
                data_dir: data,
            },
            config,
            sources: vec!["test".to_string()],
        };

        Self {
            _tempdir: tempdir,
            root,
            runtime: RuntimeDeps {
                ffmpeg,
                ffprobe: Some(ffprobe),
                whisper_cpp: whisper,
                whisper_model: model,
            },
            resolved,
            whisper_counter,
        }
    }

    pub fn ctx(&self, launcher: Arc<dyn FinalizeLauncher>) -> MeetContext {
        MeetContext::new(self.resolved.clone(), Some(self.runtime.clone())).with_launcher(launcher)
    }

    pub fn store(&self) -> meet::FileMeetingStore {
        meet::FileMeetingStore::new(&self.resolved.paths)
    }

    /// Context factory for an in-process MCP server over this harness.
    pub fn mcp_factory(
        &self,
        launcher: Arc<dyn FinalizeLauncher>,
        allow_start: bool,
    ) -> Arc<TestContextFactory> {
        let mut resolved = self.resolved.clone();
        resolved.config.mcp.allow_start = allow_start;
        Arc::new(TestContextFactory {
            resolved: Mutex::new(resolved),
            runtime: self.runtime.clone(),
            launcher,
        })
    }

    pub fn thread_launcher(&self) -> Arc<ThreadLauncher> {
        Arc::new(ThreadLauncher {
            resolved: self.resolved.clone(),
            runtime: self.runtime.clone(),
            handles: Mutex::new(Vec::new()),
        })
    }

    /// Rewrite the mock whisper so it fails for `fail_chunks` (comma-separated
    /// chunk indexes such as `00000`); an empty string makes it healthy.
    pub fn set_fail_chunks(&self, fail_chunks: &str) {
        write_executable(
            &self.runtime.whisper_cpp,
            &whisper_script(&self.whisper_counter, fail_chunks),
        );
    }

    pub fn whisper_invocations(&self) -> usize {
        fs::read_to_string(&self.whisper_counter)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    /// Start a mic-only meeting and wait until the mock recorder has written
    /// its chunks.
    pub fn start(&self, ctx: &MeetContext, chunks: usize) -> meet_service::MeetStartStatus {
        let status = meet_service::start(
            ctx,
            meet_service::StartRequest {
                mode: "raw".to_string(),
                device: ":0".to_string(),
                source: meet::MeetSourceMode::MicOnly,
                system_device: None,
                chunk_seconds: 30,
                no_llm: true,
            },
        )
        .unwrap();
        wait_for_chunks(Path::new(&status.chunks_dir), chunks);
        status
    }

    /// Start, then detach-stop with a launcher that does nothing, leaving the
    /// session `transcribing` for a test-driven finalize.
    pub fn transcribing_session(&self) -> String {
        let ctx = self.ctx(Arc::new(NoopLauncher));
        let started = self.start(&ctx, 2);
        meet_service::stop_detached(&ctx, None, Duration::from_secs(5)).unwrap();
        started.session_id
    }
}

fn whisper_script(counter: &Path, fail_chunks: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
echo invoked >> "{counter}"
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
index="${{name#chunk-}}"
fail=",{fail},"
if [[ "$fail" == *",$index,"* ]]; then
  echo "mock whisper failed for $index" >&2
  exit 7
fi
printf 'Meeting segment %s.\n' "$index" > "$out.txt"
"#,
        counter = counter.display(),
        fail = fail_chunks
    )
}

/// Runs `finalize` on a thread, as a detached process would.
pub struct ThreadLauncher {
    resolved: ResolvedConfig,
    runtime: RuntimeDeps,
    handles: Mutex<Vec<JoinHandle<Result<MeetStopStatus, ComlinkError>>>>,
}

impl ThreadLauncher {
    pub fn join_all(&self) -> Vec<Result<MeetStopStatus, ComlinkError>> {
        let handles = std::mem::take(&mut *self.handles.lock().unwrap());
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    }
}

impl FinalizeLauncher for ThreadLauncher {
    fn launch(&self, session: &MeetingSessionState) -> Result<ProcessIdentity, ComlinkError> {
        let ctx = MeetContext::new(self.resolved.clone(), Some(self.runtime.clone()))
            .with_launcher(Arc::new(FailingLauncher));
        let id = session.session_id.clone();
        let handle = thread::spawn(move || {
            meet_service::finalize(&ctx, &id, meet_service::DEFAULT_FINALIZE_LOCK_WAIT)
        });
        self.handles.lock().unwrap().push(handle);
        Ok(record::process_identity(std::process::id()))
    }
}

/// Always fails to launch, like a missing or unexecutable binary.
pub struct FailingLauncher;

impl FinalizeLauncher for FailingLauncher {
    fn launch(&self, _session: &MeetingSessionState) -> Result<ProcessIdentity, ComlinkError> {
        Err(ComlinkError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "mock spawn failure",
        )))
    }
}

/// Counts launches without starting anything.
#[derive(Default)]
pub struct CountingLauncher {
    pub launches: Mutex<usize>,
}

impl CountingLauncher {
    pub fn count(&self) -> usize {
        *self.launches.lock().unwrap()
    }
}

impl FinalizeLauncher for CountingLauncher {
    fn launch(&self, _session: &MeetingSessionState) -> Result<ProcessIdentity, ComlinkError> {
        *self.launches.lock().unwrap() += 1;
        Ok(record::process_identity(std::process::id()))
    }
}

/// Reports a launch without starting anything (a finalizer that died at once).
pub struct NoopLauncher;

impl FinalizeLauncher for NoopLauncher {
    fn launch(&self, _session: &MeetingSessionState) -> Result<ProcessIdentity, ComlinkError> {
        Ok(record::process_identity(std::process::id()))
    }
}

pub fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

pub fn wait_for_chunks(chunks_dir: &Path, expected: usize) {
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

/// Rebuilds the context from (mutable) config on every call, like the
/// production factory reloading the config file.
pub struct TestContextFactory {
    pub resolved: Mutex<ResolvedConfig>,
    pub runtime: RuntimeDeps,
    pub launcher: Arc<dyn FinalizeLauncher>,
}

impl TestContextFactory {
    pub fn set_allow_start(&self, allow_start: bool) {
        self.resolved.lock().unwrap().config.mcp.allow_start = allow_start;
    }

    pub fn set_retain_transcripts(&self, retain: bool) {
        self.resolved.lock().unwrap().config.retention.transcripts = retain;
    }
}

impl McpContextFactory for TestContextFactory {
    fn context(&self) -> Result<MeetContext, ComlinkError> {
        Ok(MeetContext::new(
            self.resolved.lock().unwrap().clone(),
            Some(self.runtime.clone()),
        )
        .with_launcher(self.launcher.clone()))
    }
}

/// Raw JSON-RPC client for an in-process `ComlinkMcp` over an in-memory
/// duplex pipe. Every line the server writes is kept in `server_lines` so
/// tests can check the wire format itself.
pub struct McpClient {
    writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    next_id: u64,
    pending: HashMap<u64, Value>,
    pub server_lines: Vec<String>,
}

impl McpClient {
    pub fn connect(factory: Arc<dyn McpContextFactory>) -> Self {
        use rmcp::ServiceExt;
        use tokio::io::AsyncBufReadExt;

        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let server = ComlinkMcp::new(factory);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        let (reader, writer) = tokio::io::split(client_io);
        Self {
            writer,
            lines: tokio::io::BufReader::new(reader).lines(),
            next_id: 1,
            pending: HashMap::new(),
            server_lines: Vec::new(),
        }
    }

    /// Connect and complete the initialize handshake at `version`.
    pub async fn initialized(factory: Arc<dyn McpContextFactory>, version: &str) -> (Self, Value) {
        let mut client = Self::connect(factory);
        let response = client.initialize(version).await;
        (client, response)
    }

    pub async fn initialize(&mut self, version: &str) -> Value {
        let response = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": version,
                    "capabilities": {},
                    "clientInfo": {"name": "comlink-tests", "version": "0"}
                }),
            )
            .await;
        self.notify("notifications/initialized", None).await;
        response
    }

    pub async fn send_raw(&mut self, line: &str) {
        use tokio::io::AsyncWriteExt;
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.write_all(b"\n").await.unwrap();
        self.writer.flush().await.unwrap();
    }

    pub async fn notify(&mut self, method: &str, params: Option<Value>) {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.send_raw(&message.to_string()).await;
    }

    /// Send a request and return the whole response message (`result` or
    /// `error`).
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send_raw(&message.to_string()).await;
        self.wait_for(id).await
    }

    async fn wait_for(&mut self, id: u64) -> Value {
        if let Some(message) = self.pending.remove(&id) {
            return message;
        }
        loop {
            let line = tokio::time::timeout(Duration::from_secs(60), self.lines.next_line())
                .await
                .expect("server response timed out")
                .unwrap()
                .expect("server closed the stream");
            self.server_lines.push(line.clone());
            let message: Value = serde_json::from_str(&line).expect("server line is JSON");
            match message.get("id").and_then(Value::as_u64) {
                Some(got) if got == id => return message,
                Some(got) => {
                    self.pending.insert(got, message);
                }
                None => {}
            }
        }
    }

    /// `tools/call`, returning the `CallToolResult`.
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await;
        response
            .get("result")
            .cloned()
            .unwrap_or_else(|| panic!("tools/call {name} was a protocol error: {response}"))
    }
}

/// Assert a tool result is a success and return its `structuredContent`.
pub fn tool_ok(result: &Value) -> Value {
    assert_eq!(
        result["isError"],
        json!(false),
        "expected success: {result:#}"
    );
    result["structuredContent"].clone()
}

/// Assert a tool result is `isError` with `error_code` and return the
/// message.
pub fn tool_err(result: &Value, code: &str) -> String {
    assert_eq!(
        result["isError"],
        json!(true),
        "expected an error: {result:#}"
    );
    assert_eq!(
        result["structuredContent"]["error_code"],
        json!(code),
        "{result:#}"
    );
    let message = result["structuredContent"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(&message), "text mirrors the message: {text}");
    message
}

/// Every line is a JSON-RPC 2.0 response or notification.
pub fn assert_json_rpc_lines<'a>(lines: impl IntoIterator<Item = &'a str>) {
    for line in lines {
        let message: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("non-JSON server output {line:?}: {error}"));
        assert_eq!(message["jsonrpc"], "2.0", "{line}");
        let is_response = message.get("id").is_some()
            && (message.get("result").is_some() ^ message.get("error").is_some());
        let is_notification = message.get("id").is_none() && message.get("method").is_some();
        assert!(
            is_response || is_notification,
            "not a JSON-RPC frame: {line}"
        );
    }
}

/// Block until `ps` no longer lists `pid` (so it was reaped, not a zombie).
pub fn assert_reaped(pid: u32, within: Duration) {
    let deadline = std::time::Instant::now() + within;
    loop {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if state.is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pid {pid} was not reaped; ps state {state:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
