//! In-process harness for the meeting service: a mock runtime whose scripts
//! have their behaviour baked in (no environment mutation), plus launcher
//! adapters for exercising detached finalize without spawning a process.

#![allow(dead_code)]

use std::{
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
    meet::{self, MeetingSessionState},
    meet_service::{self, FinalizeLauncher, MeetContext, MeetStopStatus},
    record::{self, ProcessIdentity},
};

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
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            chunks: 2,
            fail_chunks: "",
            retain_audio: false,
        }
    }
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
out="${{@: -1}}"
chunks={chunks}
mkdir -p "$(dirname "$out")"
if [ "$chunks" -gt 0 ]; then
  for index in $(seq 0 $((chunks - 1))); do
    printf 'mock wav %s\n' "$index" > "$(printf "$out" "$index")"
  done
fi
trap 'exit 0' INT TERM
while true; do
  sleep 0.1
done
"#,
                chunks = options.chunks
            ),
        );
        write_executable(&ffprobe, "#!/usr/bin/env bash\nprintf '30\\n'\n");
        write_executable(
            &whisper,
            &format!(
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
                counter = whisper_counter.display(),
                fail = options.fail_chunks
            ),
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

    pub fn thread_launcher(&self) -> Arc<ThreadLauncher> {
        Arc::new(ThreadLauncher {
            resolved: self.resolved.clone(),
            runtime: self.runtime.clone(),
            handles: Mutex::new(Vec::new()),
        })
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
