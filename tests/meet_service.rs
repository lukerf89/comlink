#![cfg(unix)]
//! In-process tests for the meeting service (`comlink::meet_service`) against
//! a mock runtime, with launcher adapters instead of real detached processes.

mod common;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

use comlink::{
    error::ComlinkError,
    meet::{self, LockWait, MeetingStatus},
    meet_service,
    record::SegmentedCaptureIdentity,
};
use common::{FailingLauncher, MockOptions, NoopLauncher, ServiceHarness};

const STOP_WAIT: Duration = Duration::from_secs(5);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

#[test]
fn detached_finalize_waits_for_lock_so_terminal_state_is_never_overwritten() {
    for _ in 0..5 {
        let harness = ServiceHarness::new(MockOptions::default());
        let launcher = harness.thread_launcher();
        let ctx = harness.ctx(launcher.clone());
        let started = harness.start(&ctx, 2);

        // The launcher starts finalize immediately, while stop_detached still
        // holds the lifecycle lock.
        let detached = meet_service::stop_detached(&ctx, None, STOP_WAIT).unwrap();
        assert_eq!(detached.status, "transcribing");
        assert_eq!(detached.finalizer_pid, std::process::id());

        let results = launcher.join_all();
        assert_eq!(results.len(), 1);
        let finalized = results.into_iter().next().unwrap().unwrap();
        assert_eq!(finalized.status, "stopped");
        assert_eq!(finalized.chunks_processed, 2);

        let session = harness.store().read_session(&started.session_id).unwrap();
        assert_eq!(session.status, MeetingStatus::Stopped);
        assert!(session.finalizer.is_none(), "finalizer must be cleared");
        assert!(session.error.is_none());
        assert_eq!(session.chunks_processed, Some(2));
        assert!(Path::new(&session.markdown_export_path).is_file());
        assert!(harness.store().active_session_id().unwrap().is_none());
    }
}

#[test]
fn finalize_reports_busy_while_another_process_holds_the_lock() {
    let harness = ServiceHarness::new(MockOptions::default());
    let id = harness.transcribing_session();
    let store = harness.store();

    let run_finalize = || {
        Command::new(env!("CARGO_BIN_EXE_comlink"))
            .args([
                "meet",
                "finalize",
                &id,
                "--format",
                "json",
                "--lock-wait-seconds",
                "1",
            ])
            .env("COMLINK_HOME", &harness.resolved.paths.home_dir)
            .env("COMLINK_DATA_DIR", &harness.resolved.paths.data_dir)
            .env("COMLINK_FFMPEG", &harness.runtime.ffmpeg)
            .env("COMLINK_FFPROBE", harness.runtime.ffprobe.as_ref().unwrap())
            .env("COMLINK_WHISPER_CPP", &harness.runtime.whisper_cpp)
            .env("COMLINK_WHISPER_MODEL", &harness.runtime.whisper_model)
            .env("COMLINK_LLM_ENABLED", "false")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };

    let lock = store
        .lock_session(&id, LockWait::Try, "test-holder")
        .unwrap();
    assert!(store.is_session_locked(&id).unwrap());
    let busy = run_finalize();
    assert_eq!(busy.status.code(), Some(1));
    assert!(busy.stdout.is_empty());
    assert!(String::from_utf8_lossy(&busy.stderr).contains("lifecycle lock"));
    assert_eq!(
        store.read_session(&id).unwrap().status,
        MeetingStatus::Transcribing,
        "a busy finalize must not change state"
    );

    // Lock is released on drop.
    drop(lock);
    assert!(!store.is_session_locked(&id).unwrap());
    let done = run_finalize();
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&done.stdout).unwrap();
    assert_eq!(status["status"], "stopped");
}

#[test]
fn launch_failure_marks_session_failed_and_finalize_can_retry() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(FailingLauncher));
    let started = harness.start(&ctx, 2);

    let error = meet_service::stop_detached(&ctx, None, STOP_WAIT).unwrap_err();
    assert!(matches!(
        error,
        ComlinkError::MeetingFinalizeLaunchFailed(_)
    ));
    assert_eq!(error.exit_code(), 1);

    let store = harness.store();
    let session = store.read_session(&started.session_id).unwrap();
    assert_eq!(session.status, MeetingStatus::Failed);
    assert!(session
        .error
        .as_deref()
        .unwrap()
        .contains("finalizer launch failed"));
    assert!(session.finalizer.is_none());
    assert!(store.active_session_id().unwrap().is_none());

    let report = meet_service::status(&ctx, Some(started.session_id.clone())).unwrap();
    assert_eq!(report.status, "failed");
    assert!(!report.stale);
    assert!(report.error.is_some());

    let finalized =
        meet_service::finalize(&ctx, &started.session_id, Duration::from_secs(1)).unwrap();
    assert_eq!(finalized.status, "stopped");
    let session = store.read_session(&started.session_id).unwrap();
    assert_eq!(session.status, MeetingStatus::Stopped);
    assert!(session.error.is_none());
}

#[test]
fn finalize_on_recording_session_is_rejected() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let started = harness.start(&ctx, 2);
    let error =
        meet_service::finalize(&ctx, &started.session_id, Duration::from_secs(1)).unwrap_err();
    assert!(matches!(error, ComlinkError::MeetingNotTranscribing(_)));
    meet_service::stop(&ctx, None, STOP_WAIT).unwrap();
}

fn revert_to_transcribing(harness: &ServiceHarness, id: &str) {
    let store = harness.store();
    let mut session = store.read_session(id).unwrap();
    session.status = MeetingStatus::Transcribing;
    store.save_session(&session).unwrap();
}

#[test]
fn crash_with_only_segments_jsonl_written_rewrites_everything() {
    let harness = ServiceHarness::new(MockOptions {
        retain_audio: true,
        ..MockOptions::default()
    });
    let id = harness.transcribing_session();
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let session = harness.store().read_session(&id).unwrap();
    fs::write(&session.segments_jsonl_path, "{\"partial\":").unwrap();

    let status = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(status.status, "stopped");
    assert_eq!(harness.whisper_invocations(), 2);
    let jsonl = fs::read_to_string(&session.segments_jsonl_path).unwrap();
    assert!(jsonl
        .lines()
        .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok()));
    assert!(Path::new(&session.json_export_path).is_file());
    assert!(Path::new(&session.markdown_export_path).is_file());
}

#[test]
fn crash_after_json_before_markdown_rewrites_when_chunks_remain() {
    let harness = ServiceHarness::new(MockOptions {
        retain_audio: true,
        ..MockOptions::default()
    });
    let id = harness.transcribing_session();
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(harness.whisper_invocations(), 2);

    let session = harness.store().read_session(&id).unwrap();
    fs::remove_file(&session.markdown_export_path).unwrap();
    revert_to_transcribing(&harness, &id);

    let status = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(status.status, "stopped");
    assert_eq!(harness.whisper_invocations(), 4, "expected a full rewrite");
    assert!(Path::new(&session.markdown_export_path).is_file());
}

#[test]
fn crash_after_all_artifacts_recovers_from_validated_export() {
    let harness = ServiceHarness::new(MockOptions {
        retain_audio: true,
        ..MockOptions::default()
    });
    let id = harness.transcribing_session();
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let first = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    let session = harness.store().read_session(&id).unwrap();
    let markdown = fs::read_to_string(&session.markdown_export_path).unwrap();
    let jsonl = fs::read_to_string(&session.segments_jsonl_path).unwrap();

    fs::write(&session.markdown_export_path, "garbage").unwrap();
    revert_to_transcribing(&harness, &id);
    let recovered = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();

    assert_eq!(
        harness.whisper_invocations(),
        2,
        "recovery must not re-run ASR"
    );
    assert_eq!(recovered.status, "stopped");
    assert_eq!(recovered.chunks_processed, first.chunks_processed);
    assert_eq!(recovered.segment_count, first.segment_count);
    assert_eq!(
        fs::read_to_string(&session.markdown_export_path).unwrap(),
        markdown
    );
    assert_eq!(
        fs::read_to_string(&session.segments_jsonl_path).unwrap(),
        jsonl
    );
    assert_eq!(
        harness.store().read_session(&id).unwrap().status,
        MeetingStatus::Stopped
    );
}

#[test]
fn retention_off_recovery_uses_export_or_fails_recoverably() {
    let harness = ServiceHarness::new(MockOptions::default());
    let id = harness.transcribing_session();
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    let session = harness.store().read_session(&id).unwrap();
    assert!(
        !Path::new(&session.chunks_dir).exists(),
        "retention off deletes chunks"
    );
    let markdown = fs::read_to_string(&session.markdown_export_path).unwrap();

    // Chunks gone, markdown missing, JSON valid: recover from the export.
    fs::remove_file(&session.markdown_export_path).unwrap();
    revert_to_transcribing(&harness, &id);
    let recovered = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(recovered.status, "stopped");
    assert_eq!(harness.whisper_invocations(), 2);
    assert_eq!(
        fs::read_to_string(&session.markdown_export_path).unwrap(),
        markdown
    );

    // Chunks gone and JSON invalid: nothing to recover from.
    fs::write(&session.json_export_path, "{ not json").unwrap();
    revert_to_transcribing(&harness, &id);
    let error = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap_err();
    assert!(
        error.to_string().contains("no recoverable export"),
        "{error}"
    );
    let failed = harness.store().read_session(&id).unwrap();
    assert_eq!(failed.status, MeetingStatus::Failed);
    assert!(failed.error.unwrap().contains("no recoverable export"));
}

#[test]
fn status_audio_level_skips_in_progress_chunk_and_warns_on_silence() {
    let harness = ServiceHarness::new(MockOptions::default());
    let store = harness.store();
    let paths = store.paths_for_new_session();
    let source = meet::MeetingSourceMetadata::new(
        meet::MeetSourceMode::MicPlusSystem,
        vec![
            stream(meet::MeetingSourceLabel::UserMic),
            stream(meet::MeetingSourceLabel::SystemAudio),
        ],
    )
    .with_session_paths(&paths.chunks_dir, &paths.recorder_stderr_path, false);
    let mut session = meet::new_recording_session(new_session(&paths, source));
    store.create_session(&session).unwrap();

    let mic_dir = PathBuf::from(session.source.streams[0].chunks_dir.clone().unwrap());
    let system_dir = PathBuf::from(session.source.streams[1].chunks_dir.clone().unwrap());
    fs::copy(fixture("short.wav"), mic_dir.join("chunk-00000.wav")).unwrap();
    // The live mic recorder's newest chunk is still being written.
    fs::write(
        mic_dir.join("chunk-00001.wav"),
        b"RIFF\x10\x00\x00\x00WAVEfmt ",
    )
    .unwrap();
    fs::copy(fixture("short.wav"), system_dir.join("chunk-00000.wav")).unwrap();
    fs::copy(fixture("silence.wav"), system_dir.join("chunk-00001.wav")).unwrap();

    // A live stand-in recorder whose command line carries the mic output
    // pattern, so the mic stream counts as live.
    let pattern = comlink::record::chunk_output_pattern(&mic_dir);
    let mut recorder = Command::new("/bin/sh")
        .args(["-c", "sleep 30; true"])
        .arg(pattern.display().to_string())
        .spawn()
        .unwrap();
    session.source.streams[0].recorder_pid = Some(recorder.id());
    session.source.streams[0].recorder_identity =
        Some(SegmentedCaptureIdentity::new(recorder.id(), &pattern));
    session.recorder_pid = Some(recorder.id());
    store.save_session(&session).unwrap();

    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let report = meet_service::status(&ctx, None).unwrap();
    recorder.kill().unwrap();
    recorder.wait().unwrap();

    assert_eq!(
        report.session_id.as_deref(),
        Some(session.session_id.as_str())
    );
    assert_eq!(report.chunk_count, 4);
    assert!(!report.stale, "one live recorder keeps the session healthy");
    assert_eq!(report.recorders.len(), 2);
    assert!(report.recorders[0].alive);
    assert!(!report.recorders[1].alive);
    // The truncated mic chunk would tie on index and win on label if it were
    // read; the silent system chunk is selected instead.
    let level = report.audio_level.expect("measurable completed chunk");
    assert!(level.near_silent);
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("near-silent")));

    // Nothing measurable (mock, non-PCM chunks) -> null level, no error.
    fs::write(system_dir.join("chunk-00001.wav"), "mock wav").unwrap();
    fs::write(system_dir.join("chunk-00000.wav"), "mock wav").unwrap();
    fs::write(mic_dir.join("chunk-00000.wav"), "mock wav").unwrap();
    let report = meet_service::status(&ctx, Some(session.session_id.clone())).unwrap();
    assert!(report.audio_level.is_none());
    assert!(report.stale, "no live recorder remains");
}

#[test]
fn list_orders_all_statuses_and_skips_corrupt_sessions() {
    let harness = ServiceHarness::new(MockOptions::default());
    let store = harness.store();
    let mut expected = Vec::new();
    for (index, (status, started_at_ms)) in [
        (MeetingStatus::Stopped, 100),
        (MeetingStatus::Transcribing, 300),
        (MeetingStatus::Failed, 200),
        (MeetingStatus::Recording, 300),
    ]
    .into_iter()
    .enumerate()
    {
        let mut session = constructed_session(&store, &format!("s{index}"));
        session.status = status;
        session.started_at_ms = started_at_ms;
        store.save_session(&session).unwrap();
        expected.push((started_at_ms, session.session_id.clone(), status.as_str()));
    }
    let corrupt = store.root().join("corrupt");
    fs::create_dir_all(&corrupt).unwrap();
    fs::write(corrupt.join("session.json"), "{").unwrap();

    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let listed = meet_service::list(&ctx).unwrap();

    expected.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    let actual = listed
        .sessions
        .iter()
        .map(|session| {
            (
                session.started_at_ms,
                session.session_id.clone(),
                session.status,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    // Equal started_at_ms is tie-broken by session id, descending.
    assert_eq!(listed.sessions[0].session_id, "s3");
    assert_eq!(listed.sessions[1].session_id, "s1");
    assert_eq!(listed.skipped.len(), 1);
    assert!(listed.skipped[0].dir.ends_with("corrupt"));
    assert!(!listed.skipped[0].reason.is_empty());
}

#[test]
fn status_resolves_active_then_newest_transcribing_then_none() {
    let harness = ServiceHarness::new(MockOptions::default());
    let store = harness.store();
    let ctx = harness.ctx(Arc::new(NoopLauncher));

    let mut older = constructed_session(&store, "t-older");
    older.status = MeetingStatus::Transcribing;
    older.started_at_ms = 100;
    store.save_session(&older).unwrap();
    let mut newer = constructed_session(&store, "t-newer");
    newer.status = MeetingStatus::Transcribing;
    newer.started_at_ms = 200;
    store.save_session(&newer).unwrap();
    let recording = constructed_session(&store, "r-active");
    store.create_session(&recording).unwrap();

    let report = meet_service::status(&ctx, None).unwrap();
    assert_eq!(report.session_id.as_deref(), Some("r-active"));
    assert_eq!(report.status, "recording");
    assert!(report.stale, "constructed session has no recorder");

    store.clear_active_if_matches("r-active").unwrap();
    let report = meet_service::status(&ctx, None).unwrap();
    assert_eq!(report.session_id.as_deref(), Some("t-newer"));
    assert_eq!(report.status, "transcribing");
    assert!(report.stale, "no lock holder and no finalizer");

    for mut session in [older, newer] {
        session.status = MeetingStatus::Stopped;
        store.save_session(&session).unwrap();
    }
    let report = meet_service::status(&ctx, None).unwrap();
    assert_eq!(report.status, "none");
    assert!(report.session_id.is_none());

    let error = meet_service::status(&ctx, Some("missing".to_string())).unwrap_err();
    assert!(matches!(error, ComlinkError::MeetingSessionNotFound(_)));
    assert_eq!(error.exit_code(), 1);
}

fn stream(label: meet::MeetingSourceLabel) -> meet::MeetingSourceStream {
    meet::MeetingSourceStream {
        label,
        device: ":0".to_string(),
        chunks_dir: None,
        recorder_stderr_path: None,
        recorder_pid: None,
        recorder_identity: None,
    }
}

fn new_session(
    paths: &meet::NewMeetingPaths,
    source: meet::MeetingSourceMetadata,
) -> meet::NewMeetingSession {
    meet::NewMeetingSession {
        session_id: paths.session_id.clone(),
        mode: "raw".to_string(),
        no_llm: true,
        device: ":0".to_string(),
        source,
        chunk_duration_ms: 30_000,
        model: "model.bin".to_string(),
        model_path: "model.bin".to_string(),
        retention: meet::MeetingRetentionPolicy {
            metadata: true,
            transcripts: true,
            audio: false,
        },
        session_dir: paths.session_dir.clone(),
        chunks_dir: paths.chunks_dir.clone(),
        recorder_stderr_path: paths.recorder_stderr_path.clone(),
        segments_jsonl_path: paths.segments_jsonl_path.clone(),
        json_export_path: paths.json_export_path.clone(),
        markdown_export_path: paths.markdown_export_path.clone(),
    }
}

fn constructed_session(store: &meet::FileMeetingStore, id: &str) -> meet::MeetingSessionState {
    let session_dir = store.root().join(id);
    let paths = meet::NewMeetingPaths {
        session_id: id.to_string(),
        chunks_dir: session_dir.join("chunks"),
        recorder_stderr_path: session_dir.join("capture.stderr"),
        segments_jsonl_path: session_dir.join("segments.jsonl"),
        json_export_path: session_dir.join("transcript.json"),
        markdown_export_path: session_dir.join("transcript.md"),
        session_dir,
    };
    let source = meet::MeetingSourceMetadata::default().with_session_paths(
        &paths.chunks_dir,
        &paths.recorder_stderr_path,
        true,
    );
    let session = meet::new_recording_session(new_session(&paths, source));
    store.save_session(&session).unwrap();
    session
}
