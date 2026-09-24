//! Meeting lifecycle service.
//!
//! Every meeting operation (`start`, `stop`, `stop_detached`, `finalize`,
//! `status`, `export`, `list`) lives here and returns typed results. This
//! module never writes to stdout or stderr: the CLI wraps these calls and does
//! all printing, and a stdio MCP server can call them without corrupting its
//! protocol stream. Warnings (near-silent capture, stale recorders, the
//! consent reminder) are returned as data.

use std::{
    fmt,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

use serde::Serialize;

use crate::{
    asr::{AsrEngine, SourceMetadata, WhisperCppEngine},
    audio,
    config::{self, ResolvedConfig},
    deps::{self, RuntimeDeps},
    error::ComlinkError,
    meet::{self, LockWait, MeetingSessionState, MeetingStatus},
    output::{self, TextProcessingResult},
    record::{self, ProcessIdentity},
    system_audio, text,
};

pub const CONSENT_REMINDER: &str =
    "Consent reminder: confirm everyone present knows this meeting is being recorded and transcribed.";
pub const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// Chunk length for long-form capture (`meet start --chunk-seconds` default).
pub const DEFAULT_CHUNK_SECONDS: u64 = 300;
/// How long `finalize` waits for the lifecycle lock (the detaching parent holds
/// it until the finalizer identity is recorded).
pub const DEFAULT_FINALIZE_LOCK_WAIT: Duration = Duration::from_secs(30);
const DETACH_LOCK_WAIT: Duration = Duration::from_secs(2);
const RECORDER_SETTLE: Duration = Duration::from_millis(250);
const FINALIZE_LOG_FILE: &str = "finalize.log";

/// Everything a meeting operation needs. `runtime` is injected by the CLI (or
/// by tests); when `None`, operations resolve it from the session's model path
/// exactly as `meet stop` always has.
pub struct MeetContext {
    pub resolved: ResolvedConfig,
    pub runtime: Option<RuntimeDeps>,
    pub launcher: Arc<dyn FinalizeLauncher>,
}

impl MeetContext {
    pub fn new(resolved: ResolvedConfig, runtime: Option<RuntimeDeps>) -> Self {
        Self {
            resolved,
            runtime,
            launcher: Arc::new(ProcessFinalizeLauncher::default()),
        }
    }

    pub fn with_launcher(mut self, launcher: Arc<dyn FinalizeLauncher>) -> Self {
        self.launcher = launcher;
        self
    }

    pub fn store(&self) -> meet::FileMeetingStore {
        meet::FileMeetingStore::new(&self.resolved.paths)
    }

    fn runtime_for_model(&self, model_path: &str) -> Result<RuntimeDeps, ComlinkError> {
        match &self.runtime {
            Some(runtime) => Ok(runtime.clone()),
            None => deps::runtime_from_model_path(PathBuf::from(model_path)),
        }
    }
}

/// Adapter that launches the detached finalizer for a session.
pub trait FinalizeLauncher: Send + Sync {
    /// Start `finalize` for `session` without waiting for it. Returns the
    /// identity of the process now responsible for it.
    fn launch(&self, session: &MeetingSessionState) -> Result<ProcessIdentity, ComlinkError>;
}

/// Spawns `<current exe> meet finalize <id> --format json` in its own process
/// group, with stdin/stdout closed and stderr appended to
/// `<session_dir>/finalize.log`. A background thread waits on the child so a
/// long-lived caller never accumulates zombie finalizers.
#[derive(Debug, Clone, Default)]
pub struct ProcessFinalizeLauncher {
    executable: Option<PathBuf>,
}

impl ProcessFinalizeLauncher {
    /// Launch `executable` instead of the current binary. It receives the same
    /// arguments (`meet finalize <id> --format json`).
    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(executable.into()),
        }
    }
}

impl FinalizeLauncher for ProcessFinalizeLauncher {
    fn launch(&self, session: &MeetingSessionState) -> Result<ProcessIdentity, ComlinkError> {
        use std::os::unix::process::CommandExt;

        let exe = match &self.executable {
            Some(executable) => executable.clone(),
            None => std::env::current_exe()?,
        };
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(finalize_log_path(session))?;
        let mut child = Command::new(exe)
            .args(["meet", "finalize", &session.session_id, "--format", "json"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .process_group(0)
            .spawn()?;
        // Read the identity before the reaper can collect the child, so the
        // pid cannot have been reused yet.
        let identity = record::process_identity(child.id());
        // Reap the finalizer when it exits. The thread only waits: it never
        // signals the child, and if this process exits first the finalizer is
        // reparented and keeps running, exactly as without the thread. If the
        // thread cannot be spawned the child is left unreaped, as before.
        let _ = std::thread::Builder::new()
            .name("comlink-finalize-reaper".to_string())
            .spawn(move || {
                let _ = child.wait();
            });
        Ok(identity)
    }
}

pub fn finalize_log_path(session: &MeetingSessionState) -> PathBuf {
    Path::new(&session.session_dir).join(FINALIZE_LOG_FILE)
}

/// Needle that identifies the finalizer for `id` in a process command line.
pub fn finalizer_command_needle(id: &str) -> String {
    format!("meet finalize {id}")
}

// ---------------------------------------------------------------------------
// Typed results
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct MeetStartStatus {
    pub schema_version: &'static str,
    pub session_id: String,
    pub status: &'static str,
    pub elapsed_ms: u64,
    pub recorder_pid: u32,
    pub recorders: Vec<MeetRecorderStatus>,
    pub source: meet::MeetingSourceMetadata,
    pub session_dir: String,
    pub chunks_dir: String,
    pub segments_jsonl: String,
    pub json_export: String,
    pub markdown_export: String,
    pub consent_reminder: &'static str,
    pub inactivity_auto_stop: meet::InactivityAutoStop,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetRecorderStatus {
    pub source_label: meet::MeetingSourceLabel,
    pub device: String,
    pub pid: u32,
    pub chunks_dir: String,
    pub stderr_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetStopStatus {
    pub schema_version: &'static str,
    pub session_id: String,
    pub status: &'static str,
    pub elapsed_ms: u64,
    pub duration_ms: u64,
    pub chunks_processed: usize,
    pub segment_count: usize,
    pub source: meet::MeetingSourceMetadata,
    pub retention: meet::MeetingRetentionPolicy,
    pub segmenting: meet::MeetingSegmenting,
    pub artifacts: meet::MeetingArtifacts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_level: Option<meet::MeetingAudioLevel>,
    pub warnings: Vec<String>,
    pub inactivity_auto_stop: meet::InactivityAutoStop,
}

/// Returned by `stop_detached`: recorders are stopped and a finalizer owns
/// transcription.
#[derive(Debug, Clone, Serialize)]
pub struct MeetDetachedStatus {
    pub schema_version: &'static str,
    pub session_id: String,
    pub status: &'static str,
    pub elapsed_ms: u64,
    pub preliminary_duration_ms: u64,
    pub chunk_count: usize,
    pub finalizer_pid: u32,
    pub artifacts: meet::MeetingArtifacts,
    pub finalize_log: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetRecorderHealth {
    pub source_label: meet::MeetingSourceLabel,
    pub device: String,
    pub pid: Option<u32>,
    pub alive: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetFinalizerHealth {
    pub pid: u32,
    pub alive: bool,
}

/// `meet status` payload. `status` is a `comlink.meeting.v1` status or `none`.
#[derive(Debug, Clone, Serialize)]
pub struct MeetStatusReport {
    pub schema_version: &'static str,
    pub session_id: Option<String>,
    pub status: &'static str,
    pub elapsed_ms: Option<u64>,
    pub recorders: Vec<MeetRecorderHealth>,
    pub finalizer: Option<MeetFinalizerHealth>,
    pub chunk_count: usize,
    pub audio_level: Option<meet::MeetingAudioLevel>,
    pub warnings: Vec<String>,
    pub stale: bool,
    pub stale_reason: Option<String>,
    pub error: Option<String>,
    /// `<session_dir>/finalize.log` when that file exists, else `null`.
    pub finalize_log: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetSessionSummary {
    pub session_id: String,
    pub status: &'static str,
    pub started_at_ms: i64,
    pub stopped_at_ms: Option<i64>,
    pub duration_ms: Option<u64>,
    pub segment_count: usize,
    pub source_mode: meet::MeetSourceMode,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedSession {
    pub dir: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetSessionList {
    pub sessions: Vec<MeetSessionSummary>,
    pub skipped: Vec<SkippedSession>,
}

/// Request for `start`. `device` is the already-resolved microphone device.
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub mode: String,
    pub device: String,
    pub source: meet::MeetSourceMode,
    pub system_device: Option<String>,
    pub chunk_seconds: u64,
    pub no_llm: bool,
}

/// Output of the pure transcription pipeline over a session's chunks.
pub struct PipelineOutput {
    pub chunks: Vec<meet::MeetingChunk>,
    pub segments: Vec<meet::MeetingSegment>,
    pub raw_text: String,
    pub processed: TextProcessingResult,
    pub segmenting: meet::MeetingSegmenting,
    pub audio_level: Option<meet::MeetingAudioLevel>,
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// prepare_start
// ---------------------------------------------------------------------------

/// Unresolved `meet start` / `meeting_start` inputs, as a user or agent gives
/// them.
#[derive(Debug, Clone)]
pub struct StartOptions {
    pub mode: String,
    /// `mic-only`, `system-only` or `mic-plus-system`.
    pub source: String,
    /// Requested microphone device; `None` uses `COMLINK_RECORD_DEVICE`, then
    /// the system default input, then `:0`.
    pub device: Option<String>,
    pub system_device: Option<String>,
    pub chunk_seconds: u64,
    pub no_llm: bool,
}

/// Returned when the microphone resolved to the system default input, so the
/// caller can tell the user which device is being recorded. Its `Display` is
/// the exact line `meet start` has always printed to stderr.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceNote {
    pub name: Option<String>,
    pub avfoundation_input: String,
}

impl fmt::Display for DeviceNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => write!(
                f,
                "Using system default input device: {name} ({})",
                self.avfoundation_input
            ),
            None => write!(
                f,
                "Using system default input device {}",
                self.avfoundation_input
            ),
        }
    }
}

/// A validated start: the resolved request, the runtime it will use, and an
/// optional device note.
#[derive(Debug, Clone)]
pub struct PreparedStart {
    pub request: StartRequest,
    pub runtime: RuntimeDeps,
    pub device_note: Option<DeviceNote>,
}

impl PreparedStart {
    /// Build the context `start` runs in, carrying the runtime resolved by
    /// [`prepare_start`] so dependencies are never resolved twice.
    pub fn into_context(
        self,
        resolved: ResolvedConfig,
    ) -> (MeetContext, StartRequest, Option<DeviceNote>) {
        (
            MeetContext::new(resolved, Some(self.runtime)),
            self.request,
            self.device_note,
        )
    }
}

/// Validate and resolve start inputs in the order `meet start` always has:
/// mode, model path, runtime dependencies, microphone device, source. The
/// first failure wins, with the same error variant as before. `runtime`
/// injects already-resolved dependencies (tests, or a caller that resolved
/// them); when `None` they are resolved from the selected model.
pub fn prepare_start(
    resolved: &ResolvedConfig,
    runtime: Option<RuntimeDeps>,
    options: StartOptions,
) -> Result<PreparedStart, ComlinkError> {
    let StartOptions {
        mode,
        source,
        device,
        system_device,
        chunk_seconds,
        no_llm,
    } = options;
    text::validate_mode(&resolved.config, &mode)?;
    let runtime = match runtime {
        Some(runtime) => runtime,
        None => {
            let model_path =
                config::selected_model_path(&resolved.config).ok_or(ComlinkError::ModelMissing)?;
            deps::runtime_from_model_path(model_path)?
        }
    };
    let device = record::resolve_record_device(device, &runtime.ffmpeg)?;
    let device_note = (device.source == record::DeviceSource::SystemDefault).then(|| DeviceNote {
        name: device.name.clone(),
        avfoundation_input: device.avfoundation_input.clone(),
    });
    let source = meet::MeetSourceMode::parse(&source).ok_or(ComlinkError::InvalidConfigValue {
        name: "meet start --source",
        value: source,
    })?;
    Ok(PreparedStart {
        request: StartRequest {
            mode,
            device: device.avfoundation_input,
            source,
            system_device,
            chunk_seconds,
            no_llm,
        },
        runtime,
        device_note,
    })
}

// ---------------------------------------------------------------------------
// start
// ---------------------------------------------------------------------------

pub fn start(ctx: &MeetContext, request: StartRequest) -> Result<MeetStartStatus, ComlinkError> {
    let StartRequest {
        mode,
        device,
        source,
        system_device,
        chunk_seconds,
        no_llm,
    } = request;
    if text::resolve_mode(&ctx.resolved.config, &mode).is_none() {
        return Err(ComlinkError::ModeNotFound(mode));
    }
    let runtime = match &ctx.runtime {
        Some(runtime) => runtime.clone(),
        None => {
            let model_path = config::selected_model_path(&ctx.resolved.config)
                .ok_or(ComlinkError::ModelMissing)?;
            deps::runtime_from_model_path(model_path)?
        }
    };
    let source_metadata =
        build_meeting_source_metadata(source, &device, system_device, &runtime.ffmpeg)?;
    let chunk_seconds = chunk_seconds.max(1);
    let store = ctx.store();

    if let Some(active_id) = store.active_session_id()? {
        match store.read_session(&active_id) {
            Ok(mut session) if session.status == MeetingStatus::Recording => {
                if session_recorder_is_verified_running(&session) {
                    return Err(ComlinkError::MeetingAlreadyActive(active_id));
                }
                reclaim_inactive_recording_session(&store, &mut session)?;
            }
            _ => store.clear_active_if_matches(&active_id)?,
        }
    }

    let paths = store.paths_for_new_session();
    let source_metadata = source_metadata.with_session_paths(
        &paths.chunks_dir,
        &paths.recorder_stderr_path,
        source == meet::MeetSourceMode::MicOnly,
    );
    let mut session = meet::new_recording_session(meet::NewMeetingSession {
        session_id: paths.session_id,
        mode,
        no_llm,
        device,
        source: source_metadata,
        chunk_duration_ms: chunk_seconds.saturating_mul(1000),
        model: runtime.whisper_model.display().to_string(),
        model_path: runtime.whisper_model.display().to_string(),
        retention: meet::MeetingRetentionPolicy::from(&ctx.resolved.config.retention),
        session_dir: paths.session_dir,
        chunks_dir: paths.chunks_dir,
        recorder_stderr_path: paths.recorder_stderr_path,
        segments_jsonl_path: paths.segments_jsonl_path,
        json_export_path: paths.json_export_path,
        markdown_export_path: paths.markdown_export_path,
    });

    store.create_session(&session)?;
    let captures = match start_session_recorders(&runtime.ffmpeg, &session, chunk_seconds) {
        Ok(captures) => captures,
        Err(error) => {
            store.clear_active_if_matches(&session.session_id)?;
            return Err(error);
        }
    };
    apply_started_recorders(&mut session, captures);
    store.save_session(&session)?;

    Ok(MeetStartStatus {
        schema_version: meet::MEETING_SCHEMA_VERSION,
        session_id: session.session_id.clone(),
        status: MeetingStatus::Recording.as_str(),
        elapsed_ms: elapsed_since(session.started_at_ms),
        recorder_pid: session.recorder_pid.unwrap_or_default(),
        recorders: session_recorders_status(&session),
        source: session.source.clone(),
        session_dir: session.session_dir.clone(),
        chunks_dir: session.chunks_dir.clone(),
        segments_jsonl: session.segments_jsonl_path.clone(),
        json_export: session.json_export_path.clone(),
        markdown_export: session.markdown_export_path.clone(),
        consent_reminder: CONSENT_REMINDER,
        inactivity_auto_stop: session.inactivity_auto_stop.clone(),
    })
}

// ---------------------------------------------------------------------------
// stop (synchronous) and stop_detached
// ---------------------------------------------------------------------------

/// Resolve the session a stop applies to (the given id, or the active one) and
/// check it is recording. Read-only; lets a caller announce the stop before
/// the slow part runs.
pub fn prepare_stop(
    ctx: &MeetContext,
    id: Option<String>,
) -> Result<MeetingSessionState, ComlinkError> {
    let store = ctx.store();
    let id = match id {
        Some(id) => id,
        None => store
            .active_session_id()?
            .ok_or(ComlinkError::MeetingNoActiveSession)?,
    };
    let session = store.read_session(&id)?;
    if session.status != MeetingStatus::Recording {
        return Err(ComlinkError::MeetingNotRecording(id));
    }
    Ok(session)
}

/// Synchronous stop: stop recorders, transcribe every chunk, write exports.
pub fn stop(
    ctx: &MeetContext,
    id: Option<String>,
    wait_timeout: Duration,
) -> Result<MeetStopStatus, ComlinkError> {
    let session = prepare_stop(ctx, id)?;
    stop_prepared(ctx, session, wait_timeout)
}

/// Synchronous stop for a session returned by [`prepare_stop`]. Keeps the
/// historical ordering: the session is marked stopped (and the active pointer
/// cleared) before ASR, so an ASR failure leaves it stopped without exports.
/// After ASR it records `chunks_processed`, writes the exports, deletes
/// unretained chunks, then saves the final `stopped`; a failed export write or
/// chunk delete leaves the session `failed` for `meet finalize` to retry.
pub fn stop_prepared(
    ctx: &MeetContext,
    mut session: MeetingSessionState,
    wait_timeout: Duration,
) -> Result<MeetStopStatus, ComlinkError> {
    let store = ctx.store();
    let _lock = store.lock_session(&session.session_id, LockWait::For(DETACH_LOCK_WAIT), "stop")?;
    // Re-read under the lock: a concurrent stop may have won the race.
    session = store.read_session(&session.session_id)?;
    if session.status != MeetingStatus::Recording {
        return Err(ComlinkError::MeetingNotRecording(session.session_id));
    }
    let stopped_at_ms = meet::now_ms();
    stop_session_recorder(&session, clamp_timeout(wait_timeout))?;
    std::thread::sleep(RECORDER_SETTLE);

    let preliminary_chunks =
        store.discover_chunks(&session, |path| audio::probe_duration_ms(path, None))?;
    let preliminary_duration_ms = meeting_duration_ms(&preliminary_chunks);
    session.mark_stopped(stopped_at_ms, preliminary_duration_ms, 0);
    store.save_session(&session)?;
    store.clear_active_if_matches(&session.session_id)?;

    if preliminary_chunks.is_empty() {
        return Err(no_meeting_chunks_error(&session));
    }

    let runtime = ctx.runtime_for_model(&session.model_path)?;
    let pipeline = transcribe_session(&store, &session, &runtime, &ctx.resolved.config)?;

    // Record progress on the preliminary `stopped` before writing artifacts,
    // so a finalize that repairs session.json from the export (after a failed
    // final save) still knows how many chunks were processed.
    session.chunks_processed = Some(pipeline.chunks.len());
    if let Err(error) = store.save_session(&session) {
        session.mark_failed(error.to_string());
        record_failed_state(&store, &session, &error);
        return Err(error);
    }

    session.mark_stopped(stopped_at_ms, pipeline.duration_ms, pipeline.segments.len());
    let export = export_from_pipeline(&session, &pipeline);

    // Same ordering as detached finalize: exports, then unretained audio, then
    // the final `stopped` save. Until that save, session.json holds the
    // preliminary `stopped` written before ASR. If an export write or the
    // chunk delete fails, the session is saved `failed` with the original
    // error, so `meet finalize <id>` retries (from the export when it is on
    // disk, else by transcribing the chunks) and commits `stopped`.
    let committed = store
        .write_segments_jsonl(&export)
        .and_then(|()| store.write_exports(&export))
        .and_then(|()| delete_unretained_chunks(&store, &session));
    if let Err(error) = committed {
        session.mark_failed(error.to_string());
        record_failed_state(&store, &session, &error);
        return Err(error);
    }
    // If this final save fails, session.json keeps the preliminary `stopped`
    // (with `chunks_processed`); `meet finalize <id>` repairs it from the
    // export.
    store.save_session(&session)?;

    Ok(stop_status_from_export(
        &session,
        export,
        pipeline.chunks.len(),
    ))
}

/// Detached stop: stop recorders, mark the session `transcribing`, launch a
/// finalizer and return without transcribing.
pub fn stop_detached(
    ctx: &MeetContext,
    id: Option<String>,
    wait_timeout: Duration,
) -> Result<MeetDetachedStatus, ComlinkError> {
    let session = prepare_stop(ctx, id)?;
    stop_detached_prepared(ctx, &session.session_id, wait_timeout)
}

pub fn stop_detached_prepared(
    ctx: &MeetContext,
    id: &str,
    wait_timeout: Duration,
) -> Result<MeetDetachedStatus, ComlinkError> {
    let store = ctx.store();
    // Held across mark_transcribing -> launch -> record finalizer -> clear
    // active, so the finalizer (which needs the same lock) cannot commit a
    // terminal state that this function would then overwrite.
    let lock = store.lock_session(id, LockWait::For(DETACH_LOCK_WAIT), "stop-detach")?;
    let mut session = store.read_session(id)?;
    if session.status != MeetingStatus::Recording {
        return Err(ComlinkError::MeetingNotRecording(id.to_string()));
    }

    let stopped_at_ms = meet::now_ms();
    stop_session_recorder(&session, clamp_timeout(wait_timeout))?;
    std::thread::sleep(RECORDER_SETTLE);

    let preliminary_chunks =
        store.discover_chunks(&session, |path| audio::probe_duration_ms(path, None))?;
    let preliminary_duration_ms = meeting_duration_ms(&preliminary_chunks);
    if preliminary_chunks.is_empty() {
        session.mark_stopped(stopped_at_ms, preliminary_duration_ms, 0);
        store.save_session(&session)?;
        store.clear_active_if_matches(&session.session_id)?;
        return Err(no_meeting_chunks_error(&session));
    }

    session.mark_transcribing(stopped_at_ms, preliminary_duration_ms);
    store.save_session(&session)?;

    match ctx.launcher.launch(&session) {
        Ok(identity) => {
            let finalizer_pid = identity.pid;
            session.finalizer = Some(identity);
            store.save_session(&session)?;
            store.clear_active_if_matches(&session.session_id)?;
            drop(lock);
            Ok(MeetDetachedStatus {
                schema_version: meet::MEETING_SCHEMA_VERSION,
                session_id: session.session_id.clone(),
                status: MeetingStatus::Transcribing.as_str(),
                elapsed_ms: elapsed_since(session.started_at_ms),
                preliminary_duration_ms,
                chunk_count: preliminary_chunks.len(),
                finalizer_pid,
                artifacts: meet::session_artifacts(&session),
                finalize_log: finalize_log_path(&session).display().to_string(),
            })
        }
        Err(error) => {
            let message = format!("finalizer launch failed: {error}");
            let error = ComlinkError::MeetingFinalizeLaunchFailed(error.to_string());
            append_finalize_log(&session, &message);
            session.mark_failed(message);
            record_failed_state(&store, &session, &error);
            if let Err(clear_error) = store.clear_active_if_matches(&session.session_id) {
                append_finalize_log(
                    &session,
                    &format!(
                        "could not clear the active-session pointer for {} ({clear_error}); original error: {error}",
                        session.session_id
                    ),
                );
            }
            drop(lock);
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------
// finalize
// ---------------------------------------------------------------------------

/// Finish a `transcribing` (or `failed`) session: transcribe, write exports,
/// delete unretained chunks, then commit `stopped`. Idempotent: on a `stopped`
/// session with a valid export it returns the same status rebuilt from that
/// export without re-transcribing, after deleting any chunks left behind while
/// `retention.audio` is off. A `stopped` session with no JSON export at all but
/// with chunks on disk (a synchronous stop whose ASR failed) is transcribed.
/// In any status, a JSON export that exists but is invalid is never
/// overwritten from the chunks: finalize returns `MeetingExportInvalid`. A
/// `stopped` session.json that disagrees with its valid export (a final save
/// that failed) is repaired from the export.
pub fn finalize(
    ctx: &MeetContext,
    id: &str,
    lock_wait: Duration,
) -> Result<MeetStopStatus, ComlinkError> {
    let store = ctx.store();
    let _lock = store.lock_session(id, LockWait::For(lock_wait), "finalize")?;
    let mut session = store.read_session(id)?;

    match session.status {
        MeetingStatus::Recording => {
            return Err(ComlinkError::MeetingNotTranscribing(id.to_string()))
        }
        MeetingStatus::Stopped => match store.validate_export_for_recovery(&session) {
            Ok(export) => {
                // A final save that failed after the export landed leaves the
                // preliminary `stopped` snapshot; bring session.json in line
                // with the validated export before reporting success.
                repair_stopped_session_from_export(&store, &mut session, &export)?;
                // Only after the export is validated on disk may chunks go;
                // regenerate a missing Markdown/JSONL from it first.
                if session_chunk_count(&store, &session)? > 0 && !session.retention.audio {
                    if !Path::new(&session.markdown_export_path).is_file() {
                        store.write_markdown_export(&export)?;
                    }
                    if !Path::new(&session.segments_jsonl_path).is_file() {
                        store.write_segments_jsonl(&export)?;
                    }
                }
                if let Err(error) = delete_unretained_chunks(&store, &session) {
                    session.mark_failed(error.to_string());
                    record_failed_state(&store, &session, &error);
                    return Err(error);
                }
                let chunks_processed = session.chunks_processed.unwrap_or_default();
                return Ok(stop_status_from_export(&session, export, chunks_processed));
            }
            // Only a stopped session with no JSON export at all (a synchronous
            // stop whose ASR failed) and chunks on disk is transcribed. A JSON
            // export that exists but does not validate is reported, never
            // overwritten: it may have been edited by the user.
            Err(error) => {
                if session_chunk_count(&store, &session)? == 0 {
                    return Err(error);
                }
                if let Some(invalid) = invalid_export_error(&session, &error) {
                    return Err(invalid);
                }
            }
        },
        MeetingStatus::Transcribing | MeetingStatus::Failed => {}
    }

    match finalize_transcribing(ctx, &store, &mut session) {
        Ok(status) => Ok(status),
        Err(error) => {
            // `stopped` is committed only as the last step, so any error here
            // leaves the session unfinished: mark it failed for a rerun.
            session.mark_failed(error.to_string());
            record_failed_state(&store, &session, &error);
            Err(error)
        }
    }
}

/// Save a session just marked `failed`. The caller returns the original
/// error; if the save itself fails, that failure is appended to the
/// session's `finalize.log` instead of replacing the original error.
fn record_failed_state(
    store: &meet::FileMeetingStore,
    session: &MeetingSessionState,
    original: &ComlinkError,
) {
    if let Err(save_error) = store.save_session(session) {
        append_finalize_log(
            session,
            &format!(
                "could not record the failed state for {} ({save_error}); original error: {original}",
                session.session_id
            ),
        );
    }
}

/// Best-effort diagnostic line in `<session_dir>/finalize.log` (error text
/// only, never transcript content).
fn append_finalize_log(session: &MeetingSessionState, line: &str) {
    use std::io::Write;

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(finalize_log_path(session))
    {
        let _ = file.write_all(format!("comlink meet finalize: {line}\n").as_bytes());
    }
}

/// When the session's JSON export exists (or cannot be ruled out) but did not
/// validate, the error finalize reports instead of overwriting it: the export
/// may have been edited by the user, so it is never silently replaced.
fn invalid_export_error(
    session: &MeetingSessionState,
    validation: &ComlinkError,
) -> Option<ComlinkError> {
    let path = Path::new(&session.json_export_path);
    if !meet::path_may_exist(path) {
        return None;
    }
    // validate_export_for_recovery reports `<path> (<reason>)`; keep the reason.
    let reason = match validation {
        ComlinkError::MeetingExportUnavailable(detail) => {
            let detail = detail.display().to_string();
            detail
                .strip_prefix(&format!("{} (", path.display()))
                .and_then(|rest| rest.strip_suffix(')'))
                .map(str::to_string)
                .unwrap_or(detail)
        }
        other => other.to_string(),
    };
    Some(ComlinkError::MeetingExportInvalid {
        id: session.session_id.clone(),
        path: path.to_path_buf(),
        reason,
    })
}

/// Rewrite a `stopped` session.json whose recorded totals disagree with its
/// validated export (the preliminary snapshot left by a synchronous stop whose
/// final save failed). A no-op when they already agree.
fn repair_stopped_session_from_export(
    store: &meet::FileMeetingStore,
    session: &mut MeetingSessionState,
    export: &meet::MeetingExport,
) -> Result<(), ComlinkError> {
    let stopped_at_ms = export.session.stopped_at_ms.or(session.stopped_at_ms);
    if session.segment_count == export.session.segment_count
        && session.duration_ms == Some(export.session.duration_ms)
        && session.stopped_at_ms == stopped_at_ms
    {
        return Ok(());
    }
    let chunks_processed = session.chunks_processed;
    session.mark_stopped(
        stopped_at_ms.unwrap_or_else(meet::now_ms),
        export.session.duration_ms,
        export.session.segment_count,
    );
    session.chunks_processed = chunks_processed;
    store.save_session(session)
}

fn session_chunk_count(
    store: &meet::FileMeetingStore,
    session: &MeetingSessionState,
) -> Result<usize, ComlinkError> {
    Ok(store
        .chunk_files(session)?
        .iter()
        .map(|(_, paths)| paths.len())
        .sum())
}

/// Delete the session's chunk WAVs when its retention policy does not keep
/// audio. A no-op when `retention.audio` is on. The error names the cleanup.
fn delete_unretained_chunks(
    store: &meet::FileMeetingStore,
    session: &MeetingSessionState,
) -> Result<(), ComlinkError> {
    if session.retention.audio {
        return Ok(());
    }
    store
        .delete_chunks(session)
        .map_err(|error| ComlinkError::MeetingChunkCleanupFailed {
            id: session.session_id.clone(),
            reason: format!("could not delete {}: {error}", session.chunks_dir),
        })
}

fn finalize_transcribing(
    ctx: &MeetContext,
    store: &meet::FileMeetingStore,
    session: &mut MeetingSessionState,
) -> Result<MeetStopStatus, ComlinkError> {
    let chunk_count = session_chunk_count(store, session)?;
    let recovered = store.validate_export_for_recovery(session);
    let artifacts_complete = Path::new(&session.markdown_export_path).is_file()
        && Path::new(&session.segments_jsonl_path).is_file();

    match &recovered {
        Ok(export) if artifacts_complete || chunk_count == 0 => {
            return recover_from_export(store, session, export.clone());
        }
        Ok(_) => {}
        // Same rule as a `stopped` session: an existing JSON export that does
        // not validate is reported, never overwritten from the chunks. (With
        // no chunks there is nothing to overwrite it from; that case is
        // reported just below.)
        Err(error) if chunk_count > 0 => {
            if let Some(invalid) = invalid_export_error(session, error) {
                return Err(invalid);
            }
        }
        Err(_) => {}
    }
    if chunk_count == 0 {
        let reason = recovered
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        return Err(ComlinkError::AudioCaptureFailed(format!(
            "meeting has no chunk files left to transcribe and no recoverable export; {reason}"
        )));
    }

    let runtime = ctx.runtime_for_model(&session.model_path)?;
    let pipeline = transcribe_session(store, session, &runtime, &ctx.resolved.config)?;
    let chunks_processed = pipeline.chunks.len();

    // Record progress before writing artifacts, so a crash after the exports
    // land can still report how many chunks were processed.
    session.chunks_processed = Some(chunks_processed);
    store.save_session(session)?;

    // Build the export from a stopped snapshot so its recorded status is
    // `stopped`; nothing is committed until every artifact is on disk.
    let mut snapshot = session.clone();
    snapshot.mark_stopped(
        session.stopped_at_ms.unwrap_or_else(meet::now_ms),
        pipeline.duration_ms,
        pipeline.segments.len(),
    );
    let export = export_from_pipeline(&snapshot, &pipeline);
    store.write_segments_jsonl(&export)?;
    store.write_exports(&export)?;
    // Exports are on disk; unretained audio goes before `stopped` is
    // committed, so a cleanup failure leaves the session retryable.
    delete_unretained_chunks(store, session)?;

    *session = snapshot;
    session.chunks_processed = Some(chunks_processed);
    store.save_session(session)?;

    Ok(stop_status_from_export(session, export, chunks_processed))
}

fn recover_from_export(
    store: &meet::FileMeetingStore,
    session: &mut MeetingSessionState,
    export: meet::MeetingExport,
) -> Result<MeetStopStatus, ComlinkError> {
    store.write_segments_jsonl(&export)?;
    store.write_markdown_export(&export)?;
    // The validated JSON export and the rewritten artifacts are on disk.
    delete_unretained_chunks(store, session)?;
    let chunks_processed = session.chunks_processed.unwrap_or_default();
    session.mark_stopped(
        export
            .session
            .stopped_at_ms
            .or(session.stopped_at_ms)
            .unwrap_or_else(meet::now_ms),
        export.session.duration_ms,
        export.session.segment_count,
    );
    session.chunks_processed = Some(chunks_processed);
    store.save_session(session)?;
    Ok(stop_status_from_export(session, export, chunks_processed))
}

// ---------------------------------------------------------------------------
// Pure transcription pipeline
// ---------------------------------------------------------------------------

/// Discover chunks, run whisper per chunk (empty transcripts are skipped),
/// stitch segments, process text and measure the audio level. Writes nothing
/// and changes no session state.
pub fn transcribe_session(
    store: &meet::FileMeetingStore,
    session: &MeetingSessionState,
    runtime: &RuntimeDeps,
    config: &config::Config,
) -> Result<PipelineOutput, ComlinkError> {
    let chunks = store.discover_chunks(session, |path| {
        audio::probe_duration_ms(path, runtime.ffprobe.as_deref())
    })?;
    if chunks.is_empty() {
        return Err(no_meeting_chunks_error(session));
    }

    let engine = WhisperCppEngine {
        binary: runtime.whisper_cpp.clone(),
        model: runtime.whisper_model.clone(),
    };
    let mut chunk_transcripts = Vec::with_capacity(chunks.len());
    for chunk in &chunks {
        let source = SourceMetadata {
            path: chunk.path.display().to_string(),
            normalized_sample_rate_hz: session.sample_rate_hz,
            normalized_channels: session.channels,
        };
        let transcript = match engine.transcribe(&chunk.path, source, chunk.duration_ms) {
            Ok(transcript) => transcript,
            Err(ComlinkError::EmptyTranscript) => continue,
            Err(error) => return Err(error),
        };
        chunk_transcripts.push(meet::ChunkTranscript {
            chunk_index: chunk.index,
            chunk_path: chunk.path.clone(),
            source_label: chunk.source_label,
            source_device: chunk.source_device.clone(),
            chunk_start_ms: chunk.start_ms,
            chunk_duration_ms: chunk.duration_ms,
            transcript,
        });
    }

    let segment_result = meet::build_segments_from_chunk_transcripts(&chunk_transcripts);
    let raw_text = meet::raw_text_from_segments(&segment_result.segments);
    let processed = output::process_text(&raw_text, &session.mode, config, session.no_llm)?;
    let duration_ms = meeting_duration_ms(&chunks);
    // Measured while the chunk WAVs are still on disk (before retention
    // cleanup) so near-silent input can be flagged.
    let audio_level = meeting_audio_level(&chunks);

    Ok(PipelineOutput {
        chunks,
        segments: segment_result.segments,
        raw_text,
        processed,
        segmenting: segment_result.segmenting,
        audio_level,
        duration_ms,
    })
}

fn export_from_pipeline(
    session: &MeetingSessionState,
    pipeline: &PipelineOutput,
) -> meet::MeetingExport {
    meet::build_export(
        session,
        &pipeline.segments,
        &pipeline.raw_text,
        &pipeline.processed,
        pipeline.segmenting.clone(),
        pipeline.audio_level,
    )
}

fn stop_status_from_export(
    session: &MeetingSessionState,
    export: meet::MeetingExport,
    chunks_processed: usize,
) -> MeetStopStatus {
    MeetStopStatus {
        schema_version: meet::MEETING_SCHEMA_VERSION,
        session_id: session.session_id.clone(),
        status: MeetingStatus::Stopped.as_str(),
        elapsed_ms: elapsed_since(session.started_at_ms),
        duration_ms: export.session.duration_ms,
        chunks_processed,
        segment_count: export.session.segment_count,
        source: export.source,
        retention: export.retention,
        segmenting: export.segmenting,
        artifacts: export.artifacts,
        audio_level: export.audio_level,
        warnings: export.warnings,
        inactivity_auto_stop: session.inactivity_auto_stop.clone(),
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

/// Read-only health report. With no id: the active recording session, else the
/// newest `transcribing` session, else `status: "none"`.
pub fn status(ctx: &MeetContext, id: Option<String>) -> Result<MeetStatusReport, ComlinkError> {
    let store = ctx.store();
    let session = match id {
        Some(id) => Some(read_session_for_status(&store, &id)?),
        None => resolve_default_status_session(&store)?,
    };
    let Some(session) = session else {
        return Ok(MeetStatusReport {
            schema_version: meet::MEETING_SCHEMA_VERSION,
            session_id: None,
            status: "none",
            elapsed_ms: None,
            recorders: Vec::new(),
            finalizer: None,
            chunk_count: 0,
            audio_level: None,
            warnings: Vec::new(),
            stale: false,
            stale_reason: None,
            error: None,
            finalize_log: None,
        });
    };
    session_status_report(&store, &session)
}

/// `read_session`, but a `session.json` that exists and cannot be read or
/// parsed is reported as [`ComlinkError::MeetingSessionUnreadable`]. A missing
/// session stays `MeetingSessionNotFound`.
fn read_session_for_status(
    store: &meet::FileMeetingStore,
    id: &str,
) -> Result<MeetingSessionState, ComlinkError> {
    store.read_session(id).map_err(|error| match error {
        ComlinkError::MeetingSessionNotFound(_) => error,
        other => ComlinkError::MeetingSessionUnreadable {
            id: id.to_string(),
            path: store.root().join(id).join("session.json"),
            reason: other.to_string(),
        },
    })
}

/// Default session for a bare `meet status`: the active recording session,
/// else the newest `transcribing` one, else the newest `failed` one when it is
/// newer than the newest `stopped` one, else none. A session whose
/// `session.json` exists but cannot be read is an error rather than `none`,
/// because its status is unknown. A session directory without a
/// `session.json` (and an active pointer to a missing session) is ignored.
fn resolve_default_status_session(
    store: &meet::FileMeetingStore,
) -> Result<Option<MeetingSessionState>, ComlinkError> {
    if let Some(active_id) = store.active_session_id()? {
        match read_session_for_status(store, &active_id) {
            Ok(session) if session.status == MeetingStatus::Recording => return Ok(Some(session)),
            Ok(_) | Err(ComlinkError::MeetingSessionNotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    let (sessions, skipped) = store.list_sessions()?;
    if let Some(session) = sessions
        .iter()
        .find(|session| session.status == MeetingStatus::Transcribing)
    {
        return Ok(Some(session.clone()));
    }
    if let Some((dir, reason)) = skipped
        .iter()
        .find(|(dir, _)| dir.join("session.json").is_file())
    {
        return Err(ComlinkError::MeetingSessionUnreadable {
            id: dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: dir.join("session.json"),
            reason: reason.clone(),
        });
    }
    // A failed finalize must not read as "nothing happening": surface the
    // newest `failed` session when it is newer than the newest stopped one.
    Ok(sessions
        .into_iter()
        .find(|session| {
            matches!(
                session.status,
                MeetingStatus::Stopped | MeetingStatus::Failed
            )
        })
        .filter(|session| session.status == MeetingStatus::Failed))
}

fn session_status_report(
    store: &meet::FileMeetingStore,
    session: &MeetingSessionState,
) -> Result<MeetStatusReport, ComlinkError> {
    let recorders = session_recorder_health(session);
    let finalizer = session
        .finalizer
        .as_ref()
        .map(|identity| MeetFinalizerHealth {
            pid: identity.pid,
            alive: record::process_identity_is_running(
                identity,
                Some(&finalizer_command_needle(&session.session_id)),
            ),
        });
    let locked = store.is_session_locked(&session.session_id)?;
    let log_path = finalize_log_path(session);
    let finalize_log = log_path.is_file().then(|| log_path.display().to_string());
    let (stale, mut stale_reason) = classify_stale(
        session.status,
        recorders.iter().any(|recorder| recorder.alive),
        locked,
        finalizer.as_ref().is_some_and(|finalizer| finalizer.alive),
        &session.session_id,
    );

    if let (Some(reason), Some(log)) = (stale_reason.as_mut(), finalize_log.as_deref()) {
        if session.status == MeetingStatus::Transcribing {
            reason.push_str(&format!("; see {log}"));
        }
    }

    let chunk_files = store.chunk_files(session)?;
    let chunk_count = chunk_files.iter().map(|(_, paths)| paths.len()).sum();
    let live_labels = recorders
        .iter()
        .filter(|recorder| recorder.alive)
        .map(|recorder| recorder.source_label)
        .collect::<Vec<_>>();
    let mut audio_level = latest_completed_chunk(&chunk_files, &live_labels)
        .and_then(|path| chunk_audio_level(&path));
    if audio_level.is_none() && session.status == MeetingStatus::Stopped {
        audio_level = store
            .validate_export_for_recovery(session)
            .ok()
            .and_then(|export| export.audio_level);
    }

    let mut warnings = Vec::new();
    if let Some(level) = &audio_level {
        if level.near_silent {
            warnings.push(audio::near_silent_warning_message(level.mean_dbfs));
        }
    }
    if let Some(reason) = &stale_reason {
        warnings.push(reason.clone());
    }
    if session.status == MeetingStatus::Failed {
        warnings.push(failed_session_warning(session, finalize_log.as_deref()));
    }

    Ok(MeetStatusReport {
        schema_version: meet::MEETING_SCHEMA_VERSION,
        session_id: Some(session.session_id.clone()),
        status: session.status.as_str(),
        elapsed_ms: Some(elapsed_since(session.started_at_ms)),
        recorders,
        finalizer,
        chunk_count,
        audio_level,
        warnings,
        stale,
        stale_reason,
        error: session.error.clone(),
        finalize_log,
    })
}

fn failed_session_warning(session: &MeetingSessionState, finalize_log: Option<&str>) -> String {
    let id = &session.session_id;
    let see_log = finalize_log
        .map(|log| format!("; see {log}"))
        .unwrap_or_default();
    format!(
        "finalize failed: {}{see_log}; rerun `comlink meet finalize {id}`",
        session.error.as_deref().unwrap_or("unknown error"),
    )
}

/// A session is stale when it claims to be active but nothing is working on
/// it: `recording` with no live recorder, or `transcribing` with the lifecycle
/// lock free and no live finalizer.
pub fn classify_stale(
    status: MeetingStatus,
    any_recorder_alive: bool,
    lifecycle_locked: bool,
    finalizer_alive: bool,
    session_id: &str,
) -> (bool, Option<String>) {
    match status {
        MeetingStatus::Recording if !any_recorder_alive && !lifecycle_locked => (
            true,
            Some(format!(
                "no recorder process is running for recording session {session_id}; run `comlink meet stop {session_id}`"
            )),
        ),
        MeetingStatus::Transcribing if !lifecycle_locked && !finalizer_alive => (
            true,
            Some(format!(
                "finalizer for {session_id} is no longer running; rerun `comlink meet finalize {session_id}`"
            )),
        ),
        _ => (false, None),
    }
}

fn session_recorder_health(session: &MeetingSessionState) -> Vec<MeetRecorderHealth> {
    let recording = session.status == MeetingStatus::Recording;
    session
        .source
        .streams
        .iter()
        .map(|stream| {
            let identity = stream.recorder_identity.clone().or_else(|| {
                stream.recorder_pid.map(|pid| {
                    let chunks_dir = stream
                        .chunks_dir
                        .as_deref()
                        .unwrap_or(session.chunks_dir.as_str());
                    record::SegmentedCaptureIdentity::new(
                        pid,
                        &record::chunk_output_pattern(Path::new(chunks_dir)),
                    )
                })
            });
            MeetRecorderHealth {
                source_label: stream.label,
                device: stream.device.clone(),
                pid: stream.recorder_pid,
                alive: recording
                    && identity
                        .as_ref()
                        .is_some_and(record::segmented_capture_is_running),
            }
        })
        .collect()
}

/// Newest chunk that is safe to read. The newest file of a stream whose
/// recorder is still live may be partially written, so it is excluded. Across
/// streams the choice is deterministic: highest per-stream index, then source
/// label.
pub fn latest_completed_chunk(
    chunk_files: &[(meet::MeetingSourceLabel, Vec<PathBuf>)],
    live_labels: &[meet::MeetingSourceLabel],
) -> Option<PathBuf> {
    chunk_files
        .iter()
        .filter_map(|(label, paths)| {
            let completed = if live_labels.contains(label) {
                paths.len().checked_sub(1)?
            } else {
                paths.len()
            };
            let index = completed.checked_sub(1)?;
            Some((index, label.as_str(), paths[index].clone()))
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)))
        .map(|(_, _, path)| path)
}

fn chunk_audio_level(path: &Path) -> Option<meet::MeetingAudioLevel> {
    let level = audio::session_audio_level(audio::read_wav_level_samples(path))?;
    Some(meet::MeetingAudioLevel {
        mean_dbfs: level.mean_dbfs,
        peak_dbfs: level.peak_dbfs,
        near_silent: level.is_near_silent(),
    })
}

// ---------------------------------------------------------------------------
// export and list
// ---------------------------------------------------------------------------

/// Read a stopped session's export. With no id: the newest stopped session,
/// unless a newer session is still transcribing or failed to finalize, which
/// is an error rather than a silent fallback to an older meeting.
pub fn export(
    ctx: &MeetContext,
    id: Option<String>,
    kind: meet::MeetingExportKind,
) -> Result<String, ComlinkError> {
    let store = ctx.store();
    let id = resolve_export_session_id(&store, id)?;
    store.read_export(&id, kind)
}

/// The session an export or transcript read applies to. An explicit id is
/// returned as is (the caller checks its status). With no id: the newest
/// session that is not recording decides — `transcribing` is
/// `MeetingStillTranscribing`, `failed` is `MeetingFinalizeFailed` — otherwise
/// the newest stopped session, else `MeetingNotStopped` for an active
/// recording, else `MeetingNoActiveSession`.
fn resolve_export_session_id(
    store: &meet::FileMeetingStore,
    id: Option<String>,
) -> Result<String, ComlinkError> {
    if let Some(id) = id {
        return Ok(id);
    }
    let (sessions, _) = store.list_sessions()?;
    let newest_finished = sessions
        .into_iter()
        .find(|session| session.status != MeetingStatus::Recording);
    if let Some(session) = newest_finished {
        match session.status {
            MeetingStatus::Transcribing => {
                return Err(ComlinkError::MeetingStillTranscribing(session.session_id))
            }
            MeetingStatus::Failed => {
                return Err(ComlinkError::MeetingFinalizeFailed(session.session_id))
            }
            _ => {}
        }
    }
    if let Some(id) = store.latest_stopped_session_id()? {
        Ok(id)
    } else if let Some(active_id) = store.active_session_id()? {
        Err(ComlinkError::MeetingNotStopped(active_id))
    } else {
        Err(ComlinkError::MeetingNoActiveSession)
    }
}

/// A stopped meeting's transcript plus the metadata an agent needs to judge
/// it (warnings such as near-silent capture, audio level, retention).
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptResult {
    pub schema_version: &'static str,
    pub session_id: String,
    pub status: &'static str,
    /// `md` or `json`.
    pub format: &'static str,
    /// Markdown export as a string, or the JSON export as an object.
    pub content: serde_json::Value,
    /// `retention.transcripts` at capture time. When false the export's
    /// transcript text is null by design; this is not an error.
    pub transcript_retained: bool,
    pub warnings: Vec<String>,
    pub audio_level: Option<meet::MeetingAudioLevel>,
}

/// Read a stopped session's transcript for an agent. Session selection is the
/// same as [`export`]; a session that is not `stopped` is an error naming its
/// state (`MeetingNotStopped` while recording, `MeetingStillTranscribing`, or
/// `MeetingFinalizeFailedDetail` with the recorded error and the finalize
/// remedy). Metadata always comes from the validated JSON export, so a missing
/// or invalid JSON export is `MeetingExportUnavailable` even for `md`.
pub fn transcript(
    ctx: &MeetContext,
    id: Option<String>,
    kind: meet::MeetingExportKind,
) -> Result<TranscriptResult, ComlinkError> {
    let store = ctx.store();
    let id = match resolve_export_session_id(&store, id) {
        Ok(id) => id,
        Err(ComlinkError::MeetingFinalizeFailed(id)) => {
            let error = store
                .read_session(&id)
                .ok()
                .and_then(|session| session.error)
                .unwrap_or_else(|| "unknown error".to_string());
            return Err(ComlinkError::MeetingFinalizeFailedDetail { id, error });
        }
        Err(error) => return Err(error),
    };
    let session = store.read_session(&id)?;
    match session.status {
        MeetingStatus::Recording => return Err(ComlinkError::MeetingNotStopped(id)),
        MeetingStatus::Transcribing => return Err(ComlinkError::MeetingStillTranscribing(id)),
        MeetingStatus::Failed => {
            return Err(ComlinkError::MeetingFinalizeFailedDetail {
                id,
                error: session
                    .error
                    .clone()
                    .unwrap_or_else(|| "unknown error".to_string()),
            })
        }
        MeetingStatus::Stopped => {}
    }
    let export = store.validate_export_for_recovery(&session)?;
    let (format, content) = match kind {
        meet::MeetingExportKind::Markdown => (
            "md",
            serde_json::Value::String(store.read_export(&id, kind)?),
        ),
        meet::MeetingExportKind::Json => {
            let text = store.read_export(&id, kind)?;
            let value = serde_json::from_str(&text).map_err(|error| {
                ComlinkError::MeetingExportUnavailable(PathBuf::from(format!(
                    "{} ({error})",
                    session.json_export_path
                )))
            })?;
            ("json", value)
        }
    };
    Ok(TranscriptResult {
        schema_version: meet::MEETING_SCHEMA_VERSION,
        session_id: id,
        status: MeetingStatus::Stopped.as_str(),
        format,
        content,
        transcript_retained: export.retention.transcripts,
        warnings: export.warnings,
        audio_level: export.audio_level,
    })
}

/// Every session in any status, newest first; unreadable session dirs are
/// reported in `skipped` rather than failing the listing.
pub fn list(ctx: &MeetContext) -> Result<MeetSessionList, ComlinkError> {
    let (sessions, skipped) = ctx.store().list_sessions()?;
    Ok(MeetSessionList {
        sessions: sessions
            .into_iter()
            .map(|session| MeetSessionSummary {
                status: session.status.as_str(),
                started_at_ms: session.started_at_ms,
                stopped_at_ms: session.stopped_at_ms,
                duration_ms: session.duration_ms,
                segment_count: session.segment_count,
                source_mode: session.source.mode,
                session_id: session.session_id,
            })
            .collect(),
        skipped: skipped
            .into_iter()
            .map(|(dir, reason)| SkippedSession {
                dir: dir.display().to_string(),
                reason,
            })
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// privacy: MCP server posture
// ---------------------------------------------------------------------------

/// What `privacy audit` and `doctor` report about the local MCP server.
#[derive(Debug, Clone, Serialize)]
pub struct McpPrivacy {
    /// Always `stdio`: the MCP client launches `comlink mcp` as a subprocess.
    pub transport: &'static str,
    /// Always false: the server opens no network socket.
    pub network_listener: bool,
    /// `mcp.allow_start`: whether MCP clients may start a recording.
    pub allow_start: bool,
    /// Always true: transcript tools and resources return transcript text to
    /// the calling model.
    pub transcripts_sent_to_calling_model: bool,
    pub note: String,
}

pub fn mcp_privacy(resolved: &ResolvedConfig) -> McpPrivacy {
    let allow_start = resolved.config.mcp.allow_start;
    let start = if allow_start {
        "MCP clients may start recordings (mcp.allow_start=true)"
    } else {
        "MCP clients cannot start recordings until `comlink config set mcp.allow_start true`"
    };
    McpPrivacy {
        transport: "stdio",
        network_listener: false,
        allow_start,
        transcripts_sent_to_calling_model: true,
        note: format!(
            "`comlink mcp` is a local stdio server launched by the MCP client and opens no network listener; {start}; meeting_get_transcript and transcript resources send transcript text to the calling model"
        ),
    }
}

// ---------------------------------------------------------------------------
// privacy: leftover unretained audio
// ---------------------------------------------------------------------------

/// A meeting session directory that may still hold unretained audio: its
/// retention policy does not keep audio (or cannot be read) and chunk WAVs are
/// on disk (or its chunks directory cannot be read).
#[derive(Debug, Clone, Serialize)]
pub struct UnretainedMeetingAudio {
    /// The session id (the directory name when `session.json` is unreadable).
    pub session_id: String,
    /// The session status, or `unknown` when `session.json` is unreadable.
    pub status: &'static str,
    /// Chunk WAVs found; `0` when the chunks directory could not be read.
    pub chunk_files: usize,
    pub chunks_dir: String,
    pub session_dir: String,
    /// Why the session is listed.
    pub reason: String,
    pub remedy: String,
}

/// A failure to scan the meetings store itself (not one session directory).
#[derive(Debug, Clone, Serialize)]
pub struct MeetingAudioScanError {
    pub path: String,
    pub reason: String,
}

/// The `meeting_audio` section of `privacy audit`. `clean` is true only when
/// no session is listed and the scan hit no error.
#[derive(Debug, Clone, Serialize)]
pub struct MeetingAudioAudit {
    pub clean: bool,
    pub unretained_leftovers: Vec<UnretainedMeetingAudio>,
    pub scan_errors: Vec<MeetingAudioScanError>,
}

/// Best-effort, conservative scan of every meeting session directory for audio
/// that the retention policy does not keep. Never fails: a directory that
/// cannot be read is listed (or recorded in `scan_errors`) instead of
/// aborting the audit. Listed:
/// - a non-`recording` session with `retention.audio = false` and chunk WAVs
///   on disk (`stopped` after a failed cleanup or from the old ordering,
///   `failed`, or still `transcribing`);
/// - a `recording` session with `retention.audio = false` and chunk WAVs whose
///   recorder is not verified running (a stale recording);
/// - any such session whose chunks directory cannot be read;
/// - a directory whose `session.json` is missing or unreadable but which holds
///   chunk WAVs or an unreadable chunks directory.
///
/// A `recording` session whose recorder is verified running is not listed:
/// its chunks are the capture in progress.
pub fn meeting_audio_audit(ctx: &MeetContext) -> MeetingAudioAudit {
    let store = ctx.store();
    let mut leftovers = Vec::new();
    let mut scan_errors = Vec::new();
    let root = store.root().to_path_buf();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return MeetingAudioAudit {
                clean: true,
                unretained_leftovers: leftovers,
                scan_errors,
            };
        }
        Err(error) => {
            scan_errors.push(MeetingAudioScanError {
                path: root.display().to_string(),
                reason: format!("could not list meeting sessions: {error}"),
            });
            return MeetingAudioAudit {
                clean: false,
                unretained_leftovers: leftovers,
                scan_errors,
            };
        }
    };

    let mut dirs = collect_session_dirs(
        &root,
        entries.map(|entry| {
            entry.map(|entry| {
                let path = entry.path();
                (path, entry.file_type().map(|kind| kind.is_dir()))
            })
        }),
        &mut scan_errors,
    );
    dirs.sort();

    for dir in dirs {
        let id = dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        match store.read_session(&id) {
            Ok(session) => {
                if let Some(leftover) = readable_session_leftover(&store, &dir, session) {
                    leftovers.push(leftover);
                }
            }
            Err(error) => {
                if let Some(leftover) = unreadable_session_leftover(&dir, &id, &error) {
                    leftovers.push(leftover);
                }
            }
        }
    }

    MeetingAudioAudit {
        clean: leftovers.is_empty() && scan_errors.is_empty(),
        unretained_leftovers: leftovers,
        scan_errors,
    }
}

/// Session directories among the store root's entries. An entry that cannot
/// be read at all is recorded against the root (it has no path yet); an entry
/// whose type cannot be read is recorded against its own path.
fn collect_session_dirs(
    root: &Path,
    entries: impl Iterator<Item = std::io::Result<(PathBuf, std::io::Result<bool>)>>,
    scan_errors: &mut Vec<MeetingAudioScanError>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for entry in entries {
        match entry {
            Ok((path, Ok(true))) => dirs.push(path),
            Ok((_, Ok(false))) => {}
            Ok((path, Err(error))) => scan_errors.push(MeetingAudioScanError {
                path: path.display().to_string(),
                reason: format!("could not read the type of a meeting session entry: {error}"),
            }),
            Err(error) => scan_errors.push(MeetingAudioScanError {
                path: root.display().to_string(),
                reason: format!("could not read a meeting session entry: {error}"),
            }),
        }
    }
    dirs
}

/// `dir` is the session directory as found on disk. Besides the chunk paths
/// recorded in session.json, WAVs under `<dir>/chunks` are counted too, so a
/// moved or restored data dir (whose recorded absolute paths point elsewhere)
/// cannot hide audio.
fn readable_session_leftover(
    store: &meet::FileMeetingStore,
    dir: &Path,
    session: MeetingSessionState,
) -> Option<UnretainedMeetingAudio> {
    if session.retention.audio {
        return None;
    }
    let stale_recording = session.status == MeetingStatus::Recording;
    if stale_recording && session_recorder_is_verified_running(&session) {
        return None;
    }
    let on_disk_chunks = dir.join("chunks");
    let recorded = store.chunk_files(&session).map(|streams| {
        streams
            .into_iter()
            .flat_map(|(_, paths)| paths)
            .collect::<Vec<_>>()
    });
    let found = wav_paths(&on_disk_chunks, 2);
    let (chunk_files, reason, unrecorded) = match (recorded, found) {
        (Err(error), _) => (
            0,
            format!(
                "unreadable chunks dir under {}: {error}",
                session.chunks_dir
            ),
            false,
        ),
        (Ok(_), Err(error)) => (
            0,
            // `error` names the path it could not read.
            format!("unreadable chunks dir: {error}"),
            false,
        ),
        (Ok(recorded), Ok(found)) => {
            let recorded_keys = recorded
                .iter()
                .map(|path| dedupe_key(path))
                .collect::<std::collections::BTreeSet<_>>();
            let unrecorded = found
                .iter()
                .filter(|path| !recorded_keys.contains(&dedupe_key(path)))
                .count();
            let count = recorded_keys.len() + unrecorded;
            if count == 0 {
                return None;
            }
            let reason = if unrecorded > 0 {
                format!(
                    "retention.audio=false and {unrecorded} chunk WAV(s) under {} are outside the chunks dir recorded in session.json ({}); was the data dir moved or restored? (status {})",
                    on_disk_chunks.display(),
                    session.chunks_dir,
                    session.status.as_str()
                )
            } else if stale_recording {
                "stale recording: retention.audio=false, chunk WAVs remain and the recorder is not verified running".to_string()
            } else {
                format!(
                    "retention.audio=false but chunk WAVs remain (status {})",
                    session.status.as_str()
                )
            };
            (count, reason, unrecorded > 0)
        }
    };
    let invalid_export = if stale_recording {
        None
    } else {
        store
            .validate_export_for_recovery(&session)
            .err()
            .and_then(|error| invalid_export_error(&session, &error))
    };
    let (reason, remedy) = if unrecorded {
        (
            reason,
            format!(
                "inspect {}; `comlink meet finalize` deletes only the recorded chunks dir {}, so remove WAVs outside it by hand",
                on_disk_chunks.display(),
                session.chunks_dir
            ),
        )
    } else if stale_recording {
        (reason, format!("comlink meet stop {}", session.session_id))
    } else if let Some(ComlinkError::MeetingExportInvalid {
        reason: invalid, ..
    }) = invalid_export
    {
        (
            format!(
                "{reason}; the JSON export at {} is invalid ({invalid}), and `comlink meet finalize` never overwrites an existing export",
                session.json_export_path
            ),
            format!(
                "fix or remove the invalid export at {}, then run `comlink meet finalize {}`",
                session.json_export_path, session.session_id
            ),
        )
    } else {
        (
            reason,
            format!("comlink meet finalize {}", session.session_id),
        )
    };
    Some(UnretainedMeetingAudio {
        status: session.status.as_str(),
        chunks_dir: session.chunks_dir.clone(),
        session_dir: session.session_dir.clone(),
        session_id: session.session_id,
        chunk_files,
        reason,
        remedy,
    })
}

/// Canonical form of a chunk path for de-duplication, or the path itself when
/// it cannot be canonicalized.
fn dedupe_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A directory whose `session.json` is missing or unreadable: its retention
/// policy is unknown, so any chunk WAV (or an unreadable chunks dir) is listed.
fn unreadable_session_leftover(
    dir: &Path,
    id: &str,
    session_error: &ComlinkError,
) -> Option<UnretainedMeetingAudio> {
    let chunks_dir = dir.join("chunks");
    let (chunk_files, reason) = match wav_paths(&chunks_dir, 2).map(|paths| paths.len()) {
        Ok(0) => return None,
        Ok(count) => (
            count,
            format!("unreadable session.json ({session_error}) and chunk WAVs remain"),
        ),
        Err(error) => (
            0,
            // `error` names the path it could not read.
            format!("unreadable session.json ({session_error}) and unreadable chunks dir: {error}"),
        ),
    };
    Some(UnretainedMeetingAudio {
        session_id: id.to_string(),
        status: "unknown",
        chunk_files,
        chunks_dir: chunks_dir.display().to_string(),
        session_dir: dir.display().to_string(),
        reason,
        remedy: format!(
            "inspect {}; repair session.json or delete the chunks directory",
            dir.display()
        ),
    })
}

/// `.wav` files in `dir` and its subdirectories down to `depth` levels
/// (per-stream chunk dirs are one level down). Only a missing `dir` counts as
/// empty; any other failure is returned with the path it concerns.
fn wav_paths(dir: &Path, depth: usize) -> std::io::Result<Vec<PathBuf>> {
    let with_path = |error: std::io::Error| {
        std::io::Error::new(error.kind(), format!("{}: {error}", dir.display()))
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(with_path(error)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(with_path)?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|error| {
            std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        if kind.is_dir() && depth > 0 {
            paths.extend(wav_paths(&path, depth - 1)?);
        } else if kind.is_file() && path.extension().and_then(|value| value.to_str()) == Some("wav")
        {
            paths.push(path);
        }
    }
    Ok(paths)
}

// ---------------------------------------------------------------------------
// Recorder helpers (moved from the CLI)
// ---------------------------------------------------------------------------

fn build_meeting_source_metadata(
    mode: meet::MeetSourceMode,
    mic_device: &str,
    system_device: Option<String>,
    ffmpeg: &Path,
) -> Result<meet::MeetingSourceMetadata, ComlinkError> {
    let system_device = if matches!(
        mode,
        meet::MeetSourceMode::SystemOnly | meet::MeetSourceMode::MicPlusSystem
    ) {
        Some(resolve_system_audio_device(system_device, ffmpeg)?)
    } else {
        None
    };

    let stream = |label, device: String| meet::MeetingSourceStream {
        label,
        device,
        chunks_dir: None,
        recorder_stderr_path: None,
        recorder_pid: None,
        recorder_identity: None,
    };

    if mode == meet::MeetSourceMode::MicPlusSystem
        && system_device
            .as_deref()
            .is_some_and(|device| same_audio_device(device, mic_device))
    {
        return Ok(meet::MeetingSourceMetadata::new(
            mode,
            vec![stream(
                meet::MeetingSourceLabel::Mixed,
                mic_device.to_string(),
            )],
        ));
    }

    let streams = match mode {
        meet::MeetSourceMode::MicOnly => vec![stream(
            meet::MeetingSourceLabel::UserMic,
            mic_device.to_string(),
        )],
        meet::MeetSourceMode::SystemOnly => vec![stream(
            meet::MeetingSourceLabel::SystemAudio,
            system_device.unwrap_or_default(),
        )],
        meet::MeetSourceMode::MicPlusSystem => vec![
            stream(meet::MeetingSourceLabel::UserMic, mic_device.to_string()),
            stream(
                meet::MeetingSourceLabel::SystemAudio,
                system_device.unwrap_or_default(),
            ),
        ],
    };

    Ok(meet::MeetingSourceMetadata::new(mode, streams))
}

fn resolve_system_audio_device(
    system_device: Option<String>,
    ffmpeg: &Path,
) -> Result<String, ComlinkError> {
    if let Some(requested) = system_device
        .or_else(|| std::env::var("COMLINK_SYSTEM_AUDIO_DEVICE").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        // Enumerate so a numeric index (`:2`) can be mapped back to its device
        // name for the BlackHole guard, alongside name-based selection.
        let devices = system_audio::list_avfoundation_audio_devices(ffmpeg)
            .map_err(ComlinkError::AudioCaptureFailed)?;
        let resolved = system_audio::resolve_avfoundation_audio_device(&requested, &devices)
            .map_err(|error| ComlinkError::AudioCaptureFailed(error.to_string()))?;
        // The resolved name is only known when we matched an enumerated device;
        // fall back to the raw request so the guard message stays informative.
        let device_name = resolved.name.as_deref().unwrap_or(requested.as_str());
        if !device_name.to_ascii_lowercase().contains("blackhole") {
            return Err(ComlinkError::AudioCaptureFailed(format!(
                "system-audio capture requires a BlackHole input device; got `{device_name}`. Install BlackHole 2ch or set COMLINK_SYSTEM_AUDIO_DEVICE to the exact BlackHole AVFoundation input name."
            )));
        }

        return Ok(resolved.avfoundation_input);
    }

    let probe = system_audio::RealSystemAudioProbe::from_ffmpeg(ffmpeg.to_path_buf());
    system_audio::resolve_capture_plan(&probe)
        .map(|plan| plan.device_name)
        .map_err(ComlinkError::AudioCaptureFailed)
}

fn same_audio_device(left: &str, right: &str) -> bool {
    left.trim_start_matches(':')
        .eq_ignore_ascii_case(right.trim_start_matches(':'))
}

fn start_session_recorders(
    ffmpeg: &Path,
    session: &MeetingSessionState,
    chunk_seconds: u64,
) -> Result<Vec<(meet::MeetingSourceLabel, record::SegmentedCapture)>, ComlinkError> {
    let mut captures: Vec<(meet::MeetingSourceLabel, record::SegmentedCapture)> =
        Vec::with_capacity(session.source.streams.len());
    for stream in &session.source.streams {
        let chunks_dir = stream.chunks_dir.as_deref().ok_or_else(|| {
            ComlinkError::AudioCaptureFailed("missing stream chunks_dir".to_string())
        })?;
        let stderr_path = stream.recorder_stderr_path.as_deref().ok_or_else(|| {
            ComlinkError::AudioCaptureFailed("missing stream recorder stderr path".to_string())
        })?;
        let device = capture_device_for_stream(stream);
        let capture = match record::start_segmented_capture(record::SegmentedCaptureOptions {
            ffmpeg,
            device: &device,
            chunks_dir: Path::new(chunks_dir),
            stderr_path: Path::new(stderr_path),
            chunk_duration: Duration::from_secs(chunk_seconds),
        }) {
            Ok(capture) => capture,
            Err(error) => {
                for (_, capture) in &captures {
                    let _ = record::stop_segmented_capture(&capture.identity, DEFAULT_STOP_TIMEOUT);
                }
                return Err(error);
            }
        };
        captures.push((stream.label, capture));
    }
    Ok(captures)
}

fn capture_device_for_stream(stream: &meet::MeetingSourceStream) -> String {
    match stream.label {
        meet::MeetingSourceLabel::SystemAudio => {
            system_audio::avfoundation_audio_input(&stream.device)
        }
        meet::MeetingSourceLabel::Mixed
            if stream.device.to_ascii_lowercase().contains("blackhole") =>
        {
            system_audio::avfoundation_audio_input(&stream.device)
        }
        _ => stream.device.clone(),
    }
}

fn apply_started_recorders(
    session: &mut MeetingSessionState,
    captures: Vec<(meet::MeetingSourceLabel, record::SegmentedCapture)>,
) {
    for (label, capture) in captures {
        if session.recorder_pid.is_none() {
            session.recorder_pid = Some(capture.pid);
            session.recorder_identity = Some(capture.identity.clone());
        }
        if let Some(stream) = session
            .source
            .streams
            .iter_mut()
            .find(|stream| stream.label == label)
        {
            stream.recorder_pid = Some(capture.pid);
            stream.recorder_identity = Some(capture.identity);
        }
    }
}

fn session_recorders_status(session: &MeetingSessionState) -> Vec<MeetRecorderStatus> {
    session
        .source
        .streams
        .iter()
        .filter_map(|stream| {
            Some(MeetRecorderStatus {
                source_label: stream.label,
                device: stream.device.clone(),
                pid: stream.recorder_pid?,
                chunks_dir: stream
                    .chunks_dir
                    .clone()
                    .unwrap_or_else(|| session.chunks_dir.clone()),
                stderr_path: stream
                    .recorder_stderr_path
                    .clone()
                    .unwrap_or_else(|| session.recorder_stderr_path.clone()),
            })
        })
        .collect()
}

fn session_recorder_identities(
    session: &MeetingSessionState,
) -> Vec<record::SegmentedCaptureIdentity> {
    let identities = session
        .source
        .streams
        .iter()
        .filter_map(|stream| {
            if let Some(identity) = &stream.recorder_identity {
                return Some(identity.clone());
            }
            stream.recorder_pid.map(|pid| {
                let chunks_dir = stream
                    .chunks_dir
                    .as_deref()
                    .unwrap_or(session.chunks_dir.as_str());
                let output_pattern = record::chunk_output_pattern(Path::new(chunks_dir));
                record::SegmentedCaptureIdentity::new(pid, &output_pattern)
            })
        })
        .collect::<Vec<_>>();

    if !identities.is_empty() {
        return identities;
    }

    session
        .recorder_pid
        .map(|pid| {
            let output_pattern = record::chunk_output_pattern(Path::new(&session.chunks_dir));
            record::SegmentedCaptureIdentity::new(pid, &output_pattern)
        })
        .into_iter()
        .collect()
}

pub fn session_recorder_is_verified_running(session: &MeetingSessionState) -> bool {
    let identities = session_recorder_identities(session);
    !identities.is_empty() && identities.iter().any(record::segmented_capture_is_running)
}

fn reclaim_inactive_recording_session(
    store: &meet::FileMeetingStore,
    session: &mut MeetingSessionState,
) -> Result<(), ComlinkError> {
    let duration_ms = store
        .discover_chunks(session, |path| audio::probe_duration_ms(path, None))
        .ok()
        .map(|chunks| meeting_duration_ms(&chunks))
        .unwrap_or_default();
    session.mark_stopped(meet::now_ms(), duration_ms, 0);
    store.save_session(session)?;
    store.clear_active_if_matches(&session.session_id)
}

fn stop_session_recorder(
    session: &MeetingSessionState,
    timeout: Duration,
) -> Result<bool, ComlinkError> {
    let identities = session_recorder_identities(session);
    if identities.is_empty() {
        return Ok(false);
    }
    let mut stopped_any = false;
    for identity in identities {
        stopped_any |= record::stop_segmented_capture(&identity, timeout)?;
    }
    Ok(stopped_any)
}

fn recorder_stderr_paths(session: &MeetingSessionState) -> Vec<String> {
    let mut paths = Vec::new();
    for stream in &session.source.streams {
        if let Some(path) = &stream.recorder_stderr_path {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
    }

    if paths.is_empty() && !session.recorder_stderr_path.is_empty() {
        paths.push(session.recorder_stderr_path.clone());
    }

    paths
}

fn no_meeting_chunks_error(session: &MeetingSessionState) -> ComlinkError {
    let paths = recorder_stderr_paths(session);
    let stderr_hint = match paths.as_slice() {
        [] => "no recorder stderr log was recorded".to_string(),
        [path] => format!("see recorder stderr log: {path}"),
        _ => format!("see recorder stderr logs: {}", paths.join(", ")),
    };

    ComlinkError::AudioCaptureFailed(format!(
        "meeting recording produced no chunk files; {stderr_hint}"
    ))
}

/// Combine the per-chunk WAV level measurements into a single session-level
/// audio level, returning `None` when nothing measurable was captured (e.g. the
/// chunk files are not 16-bit PCM). Measurement is best-effort: chunks that fail
/// to read are simply skipped rather than failing the stop.
fn meeting_audio_level(chunks: &[meet::MeetingChunk]) -> Option<meet::MeetingAudioLevel> {
    let measurements = chunks
        .iter()
        .filter_map(|chunk| audio::read_wav_level_samples(&chunk.path));
    let level = audio::session_audio_level(measurements)?;
    Some(meet::MeetingAudioLevel {
        mean_dbfs: level.mean_dbfs,
        peak_dbfs: level.peak_dbfs,
        near_silent: level.is_near_silent(),
    })
}

fn meeting_duration_ms(chunks: &[meet::MeetingChunk]) -> u64 {
    chunks
        .iter()
        .map(meet::MeetingChunk::end_ms)
        .max()
        .unwrap_or_default()
}

fn elapsed_since(started_at_ms: i64) -> u64 {
    meet::now_ms().saturating_sub(started_at_ms) as u64
}

fn clamp_timeout(timeout: Duration) -> Duration {
    timeout.max(Duration::from_secs(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meet::MeetingSourceLabel;

    #[test]
    fn service_source_never_prints() {
        let source = include_str!("meet_service.rs");
        let forbidden = ["print", "ln!(", "eprint", "dbg"];
        // Build the banned macro names at runtime so this guard does not match
        // itself.
        let banned = [
            format!("{}{}", forbidden[0], "ln!("),
            format!("{}{}", forbidden[0], "!("),
            format!("{}{}", forbidden[2], forbidden[1]),
            format!("{}{}", forbidden[2], "!("),
            format!("{}{}", forbidden[3], "!("),
        ];
        for macro_name in banned {
            assert!(
                !source.contains(&macro_name),
                "meet_service.rs must not call {macro_name}"
            );
        }
    }

    #[test]
    fn stale_classification_table() {
        use MeetingStatus::*;
        let cases = [
            // status, recorder alive, locked, finalizer alive, expected stale
            (Recording, true, false, false, false),
            (Recording, false, false, false, true),
            (Recording, false, true, false, false),
            (Transcribing, false, true, false, false),
            (Transcribing, false, false, true, false),
            (Transcribing, false, false, false, true),
            (Stopped, false, false, false, false),
            (Failed, false, false, false, false),
        ];
        for (status, recorder, locked, finalizer, expected) in cases {
            let (stale, reason) = classify_stale(status, recorder, locked, finalizer, "s1");
            assert_eq!(
                stale, expected,
                "{status:?} {recorder} {locked} {finalizer}"
            );
            assert_eq!(reason.is_some(), expected);
        }
        let (_, reason) = classify_stale(Transcribing, false, false, false, "s1");
        assert!(reason.unwrap().contains("comlink meet finalize s1"));
    }

    #[test]
    fn latest_completed_chunk_skips_live_newest_and_is_deterministic() {
        let mic = (
            MeetingSourceLabel::UserMic,
            vec![PathBuf::from("mic/0.wav"), PathBuf::from("mic/1.wav")],
        );
        let system = (
            MeetingSourceLabel::SystemAudio,
            vec![PathBuf::from("sys/0.wav"), PathBuf::from("sys/1.wav")],
        );
        let files = vec![mic.clone(), system.clone()];

        // Nothing live: highest index wins, ties broken by the larger label
        // string (`user_mic` > `system_audio`).
        assert_eq!(
            latest_completed_chunk(&files, &[]),
            Some(PathBuf::from("mic/1.wav"))
        );
        // Both live: newest per stream excluded.
        assert_eq!(
            latest_completed_chunk(
                &files,
                &[MeetingSourceLabel::UserMic, MeetingSourceLabel::SystemAudio]
            ),
            Some(PathBuf::from("mic/0.wav"))
        );
        // Only mic live: system's newest is complete and has the higher index.
        assert_eq!(
            latest_completed_chunk(&files, &[MeetingSourceLabel::UserMic]),
            Some(PathBuf::from("sys/1.wav"))
        );
        // A single in-progress chunk is never read.
        let single = vec![(MeetingSourceLabel::UserMic, vec![PathBuf::from("0.wav")])];
        assert_eq!(
            latest_completed_chunk(&single, &[MeetingSourceLabel::UserMic]),
            None
        );
        assert_eq!(latest_completed_chunk(&[], &[]), None);
    }

    #[test]
    fn session_dir_scan_names_the_entry_whose_type_cannot_be_read() {
        let root = Path::new("/store");
        let denied = || std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let entries = vec![
            Ok((root.join("a"), Ok(true))),
            Ok((root.join("file"), Ok(false))),
            Ok((root.join("b"), Err(denied()))),
            Err(denied()),
        ];
        let mut errors = Vec::new();
        let dirs = collect_session_dirs(root, entries.into_iter(), &mut errors);
        assert_eq!(dirs, vec![root.join("a")]);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].path, "/store/b");
        assert_eq!(errors[1].path, "/store");
    }
}
