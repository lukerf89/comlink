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
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("finalize failed")
                && warning.contains("finalize.log")
                && warning.contains(&format!("comlink meet finalize {}", started.session_id))),
        "{:?}",
        report.warnings
    );

    // A bare `meet status` surfaces the failed session instead of `none`.
    let report = meet_service::status(&ctx, None).unwrap();
    assert_eq!(
        report.session_id.as_deref(),
        Some(started.session_id.as_str())
    );
    assert_eq!(report.status, "failed");
    assert!(!report.warnings.is_empty());

    // A bare `meet export` refuses rather than exporting an older meeting.
    let error = meet_service::export(&ctx, None, meet::MeetingExportKind::Markdown).unwrap_err();
    assert!(
        matches!(&error, ComlinkError::MeetingFinalizeFailed(id) if id == &started.session_id),
        "{error}"
    );

    let finalized =
        meet_service::finalize(&ctx, &started.session_id, Duration::from_secs(1)).unwrap();
    assert_eq!(finalized.status, "stopped");
    let session = store.read_session(&started.session_id).unwrap();
    assert_eq!(session.status, MeetingStatus::Stopped);
    assert!(session.error.is_none());
}

#[test]
fn bare_export_after_detached_stop_does_not_fall_back_to_older_meeting() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let store = harness.store();

    // An earlier, fully stopped meeting.
    let first = harness.start(&ctx, 2);
    meet_service::stop(&ctx, None, STOP_WAIT).unwrap();
    let mut earlier = store.read_session(&first.session_id).unwrap();
    earlier.started_at_ms -= 60_000;
    store.save_session(&earlier).unwrap();
    assert!(
        meet_service::export(&ctx, None, meet::MeetingExportKind::Markdown).is_ok(),
        "the stopped meeting exports while it is the newest"
    );

    // A newer meeting stopped with --detach is still transcribing.
    let second = harness.start(&ctx, 2);
    let detached = meet_service::stop_detached(&ctx, None, STOP_WAIT).unwrap();
    assert_eq!(detached.status, "transcribing");
    let error = meet_service::export(&ctx, None, meet::MeetingExportKind::Markdown).unwrap_err();
    assert!(
        matches!(&error, ComlinkError::MeetingStillTranscribing(id) if id == &second.session_id),
        "{error}"
    );
    assert_eq!(error.exit_code(), 1);
    assert!(error
        .to_string()
        .contains(&format!("comlink meet status {}", second.session_id)));

    // Once finalized, the bare export resolves to the newer meeting.
    meet_service::finalize(&ctx, &second.session_id, Duration::from_secs(1)).unwrap();
    let json = meet_service::export(&ctx, None, meet::MeetingExportKind::Json).unwrap();
    assert!(json.contains(&second.session_id));
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

    for mut session in [older.clone(), newer.clone()] {
        session.status = MeetingStatus::Stopped;
        store.save_session(&session).unwrap();
    }
    let report = meet_service::status(&ctx, None).unwrap();
    assert_eq!(report.status, "none");
    assert!(report.session_id.is_none());

    // A failed session newer than every stopped one is reported, not `none`.
    let mut failed = newer.clone();
    failed.status = MeetingStatus::Failed;
    failed.error = Some("mock whisper failure".to_string());
    store.save_session(&failed).unwrap();
    let report = meet_service::status(&ctx, None).unwrap();
    assert_eq!(report.session_id.as_deref(), Some("t-newer"));
    assert_eq!(report.status, "failed");
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("finalize failed: mock whisper failure")));

    // An older failed session behind a newer stopped one is not.
    failed.started_at_ms = 50;
    store.save_session(&failed).unwrap();
    let report = meet_service::status(&ctx, None).unwrap();
    assert_eq!(report.status, "none");

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

// ---------------------------------------------------------------------------
// LF-161 fix round
// ---------------------------------------------------------------------------

fn set_mode(path: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn wav_count(dir: &str) -> usize {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().and_then(|v| v.to_str()) == Some("wav"))
                .count()
        })
        .unwrap_or(0)
}

/// Puts chunk WAVs back, as a session finalized by the old
/// stopped-before-cleanup ordering (or whose cleanup failed) would have left.
fn plant_leftover_chunks(session: &meet::MeetingSessionState) {
    fs::create_dir_all(&session.chunks_dir).unwrap();
    for index in 0..2 {
        fs::write(
            Path::new(&session.chunks_dir).join(format!("chunk-0000{index}.wav")),
            "leftover",
        )
        .unwrap();
    }
}

#[test]
fn finalize_cleanup_failure_is_not_stopped_and_rerun_cleans_up() {
    let harness = ServiceHarness::new(MockOptions::default());
    let id = harness.transcribing_session();
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let store = harness.store();
    let session = store.read_session(&id).unwrap();

    // Read-only chunks dir: its WAVs cannot be unlinked.
    set_mode(&session.chunks_dir, 0o555);
    let error = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap_err();
    set_mode(&session.chunks_dir, 0o755);
    assert!(
        matches!(&error, ComlinkError::MeetingChunkCleanupFailed { id: failed, .. } if failed == &id),
        "{error}"
    );
    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("cleanup"), "{error}");

    let failed = store.read_session(&id).unwrap();
    assert_eq!(failed.status, MeetingStatus::Failed, "must not be stopped");
    assert!(failed.error.as_deref().unwrap().contains("cleanup"));
    assert_eq!(wav_count(&session.chunks_dir), 2, "audio is still on disk");
    // Exports were written before the cleanup was attempted.
    assert!(store.validate_export_for_recovery(&session).is_ok());
    let leftovers = meet_service::meeting_audio_audit(&ctx).unretained_leftovers;
    assert_eq!(leftovers.len(), 1);
    assert_eq!(leftovers[0].session_id, id);
    assert_eq!(leftovers[0].status, "failed");
    assert_eq!(leftovers[0].chunk_files, 2);

    // Rerun: recovers from the export (no ASR), deletes the audio, stops.
    let status = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(status.status, "stopped");
    assert_eq!(status.chunks_processed, 2);
    assert_eq!(
        harness.whisper_invocations(),
        2,
        "rerun must not re-run ASR"
    );
    assert!(!Path::new(&session.chunks_dir).exists(), "audio is gone");
    let stopped = store.read_session(&id).unwrap();
    assert_eq!(stopped.status, MeetingStatus::Stopped);
    assert!(stopped.error.is_none());
    assert!(meet_service::meeting_audio_audit(&ctx).clean);
}

#[test]
fn stopped_session_with_leftover_chunks_is_cleaned_by_finalize() {
    let harness = ServiceHarness::new(MockOptions::default());
    let id = harness.transcribing_session();
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let store = harness.store();
    meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    let session = store.read_session(&id).unwrap();
    assert_eq!(session.status, MeetingStatus::Stopped);
    plant_leftover_chunks(&session);

    let leftovers = meet_service::meeting_audio_audit(&ctx).unretained_leftovers;
    assert_eq!(leftovers.len(), 1);
    assert_eq!(leftovers[0].status, "stopped");
    assert!(leftovers[0].remedy.contains(&format!("meet finalize {id}")));

    // Cleanup failure on the stopped fast path: not left `stopped`.
    set_mode(&session.chunks_dir, 0o555);
    let error = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap_err();
    set_mode(&session.chunks_dir, 0o755);
    assert!(
        matches!(error, ComlinkError::MeetingChunkCleanupFailed { .. }),
        "{error}"
    );
    assert_eq!(
        store.read_session(&id).unwrap().status,
        MeetingStatus::Failed
    );
    assert_eq!(wav_count(&session.chunks_dir), 2);

    let status = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(status.status, "stopped");
    assert!(!Path::new(&session.chunks_dir).exists());
    assert_eq!(harness.whisper_invocations(), 2);
    assert!(meet_service::meeting_audio_audit(&ctx).clean);

    // A stopped session with leftovers and a healthy disk: the fast path
    // regenerates a missing Markdown export from the JSON, then deletes the
    // leftovers, and the session stays stopped.
    let markdown = fs::read_to_string(&session.markdown_export_path).unwrap();
    fs::remove_file(&session.markdown_export_path).unwrap();
    plant_leftover_chunks(&session);
    let status = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(status.status, "stopped");
    assert!(!Path::new(&session.chunks_dir).exists());
    assert_eq!(
        store.read_session(&id).unwrap().status,
        MeetingStatus::Stopped
    );
    assert_eq!(
        fs::read_to_string(&session.markdown_export_path).unwrap(),
        markdown
    );
    assert_eq!(harness.whisper_invocations(), 2);
}

#[test]
fn retained_audio_is_never_deleted_by_finalize() {
    let harness = ServiceHarness::new(MockOptions {
        retain_audio: true,
        ..MockOptions::default()
    });
    let id = harness.transcribing_session();
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let session = harness.store().read_session(&id).unwrap();

    meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(wav_count(&session.chunks_dir), 2);
    // Stopped fast path, and recovery from the export after a crash.
    meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(wav_count(&session.chunks_dir), 2);
    revert_to_transcribing(&harness, &id);
    meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    assert_eq!(wav_count(&session.chunks_dir), 2);
    assert!(meet_service::meeting_audio_audit(&ctx).clean);
}

#[test]
fn finalize_save_failure_keeps_the_original_error_and_logs_the_save_failure() {
    let harness = ServiceHarness::new(MockOptions {
        fail_chunks: "00000",
        ..MockOptions::default()
    });
    let id = harness.transcribing_session();
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let session = harness.store().read_session(&id).unwrap();
    let log = meet_service::finalize_log_path(&session);
    fs::write(&log, "").unwrap();

    // A read-only session dir: session.json cannot be rewritten.
    set_mode(&session.session_dir, 0o555);
    let error = meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap_err();
    set_mode(&session.session_dir, 0o755);

    assert!(matches!(error, ComlinkError::WhisperFailed(_)), "{error}");
    assert_eq!(error.exit_code(), 3);
    let logged = fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains("could not record the failed state") && logged.contains("whisper.cpp"),
        "{logged}"
    );
    assert!(!logged.contains("Meeting segment"), "log leaked transcript");
}

#[test]
fn sync_stop_records_chunks_processed_for_finalize() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let started = harness.start(&ctx, 2);
    let stopped = meet_service::stop(&ctx, None, STOP_WAIT).unwrap();
    assert_eq!(stopped.chunks_processed, 2);
    let session = harness.store().read_session(&started.session_id).unwrap();
    assert_eq!(session.chunks_processed, Some(2));
    let finalized =
        meet_service::finalize(&ctx, &started.session_id, Duration::from_secs(1)).unwrap();
    assert_eq!(finalized.chunks_processed, 2);
}

#[test]
fn stop_detach_without_chunks_stops_and_launches_nothing() {
    let harness = ServiceHarness::new(MockOptions {
        chunks: 0,
        ..MockOptions::default()
    });
    let launcher = Arc::new(common::CountingLauncher::default());
    let ctx = harness.ctx(launcher.clone());
    let started = harness.start(&ctx, 0);

    let error = meet_service::stop_detached(&ctx, None, STOP_WAIT).unwrap_err();
    assert!(
        matches!(error, ComlinkError::AudioCaptureFailed(_)),
        "{error}"
    );
    assert_eq!(error.exit_code(), 2);
    assert_eq!(launcher.count(), 0, "no finalizer for an empty meeting");
    let store = harness.store();
    let session = store.read_session(&started.session_id).unwrap();
    assert_eq!(session.status, MeetingStatus::Stopped);
    assert!(session.finalizer.is_none());
    assert!(store.active_session_id().unwrap().is_none());
}

#[test]
fn stop_that_lost_the_race_does_not_transcribe_or_launch_again() {
    // Synchronous: a stop prepared before another stop won the race.
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    harness.start(&ctx, 2);
    let prepared = meet_service::prepare_stop(&ctx, None).unwrap();
    meet_service::stop(&ctx, Some(prepared.session_id.clone()), STOP_WAIT).unwrap();
    assert_eq!(harness.whisper_invocations(), 2);
    let error = meet_service::stop_prepared(&ctx, prepared, STOP_WAIT).unwrap_err();
    assert!(
        matches!(error, ComlinkError::MeetingNotRecording(_)),
        "{error}"
    );
    assert_eq!(harness.whisper_invocations(), 2, "transcribed twice");

    // Detached: the loser must not launch a second finalizer.
    let harness = ServiceHarness::new(MockOptions::default());
    let launcher = Arc::new(common::CountingLauncher::default());
    let ctx = harness.ctx(launcher.clone());
    harness.start(&ctx, 2);
    let prepared = meet_service::prepare_stop(&ctx, None).unwrap();
    meet_service::stop_detached_prepared(&ctx, &prepared.session_id, STOP_WAIT).unwrap();
    let error =
        meet_service::stop_detached_prepared(&ctx, &prepared.session_id, STOP_WAIT).unwrap_err();
    assert!(
        matches!(error, ComlinkError::MeetingNotRecording(_)),
        "{error}"
    );
    assert_eq!(launcher.count(), 1, "second finalizer launched");
    assert_eq!(
        harness
            .store()
            .read_session(&prepared.session_id)
            .unwrap()
            .status,
        MeetingStatus::Transcribing
    );
}

#[test]
fn sync_stop_asr_failure_leaves_stopped_without_exports_and_finalize_recovers() {
    let harness = ServiceHarness::new(MockOptions {
        fail_chunks: "00000",
        ..MockOptions::default()
    });
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let started = harness.start(&ctx, 2);
    let store = harness.store();

    let error = meet_service::stop(&ctx, None, STOP_WAIT).unwrap_err();
    assert!(matches!(error, ComlinkError::WhisperFailed(_)), "{error}");
    // Pinned sync behaviour: stopped, no exports, chunks kept, not active.
    let session = store.read_session(&started.session_id).unwrap();
    assert_eq!(session.status, MeetingStatus::Stopped);
    assert!(!Path::new(&session.json_export_path).exists());
    assert_eq!(wav_count(&session.chunks_dir), 2);
    assert!(store.active_session_id().unwrap().is_none());
    assert_eq!(
        meet_service::status(&ctx, None).unwrap().status,
        "none",
        "known limitation: bare status does not surface it"
    );
    let leftovers = meet_service::meeting_audio_audit(&ctx).unretained_leftovers;
    assert_eq!(leftovers.len(), 1, "privacy audit sees the kept audio");

    // finalize transcribes it; a failure there marks it failed.
    let error =
        meet_service::finalize(&ctx, &started.session_id, Duration::from_secs(1)).unwrap_err();
    assert!(matches!(error, ComlinkError::WhisperFailed(_)), "{error}");
    assert_eq!(
        store.read_session(&started.session_id).unwrap().status,
        MeetingStatus::Failed
    );
    harness.set_fail_chunks("");
    let status = meet_service::finalize(&ctx, &started.session_id, Duration::from_secs(1)).unwrap();
    assert_eq!(status.status, "stopped");
    assert_eq!(status.chunks_processed, 2);
    assert!(Path::new(&session.markdown_export_path).is_file());
    assert!(!Path::new(&session.chunks_dir).exists());
}

#[test]
fn status_surfaces_unreadable_session_instead_of_none() {
    let harness = ServiceHarness::new(MockOptions::default());
    let store = harness.store();
    let ctx = harness.ctx(Arc::new(NoopLauncher));

    // A directory without session.json is not a session: still `none`.
    fs::create_dir_all(store.root().join("not-a-session")).unwrap();
    assert_eq!(meet_service::status(&ctx, None).unwrap().status, "none");
    // An active pointer to a missing session is ignored.
    fs::write(store.root().join("active-session"), "gone").unwrap();
    assert_eq!(meet_service::status(&ctx, None).unwrap().status, "none");

    let mut stopped = constructed_session(&store, "old-stopped");
    stopped.status = MeetingStatus::Stopped;
    store.save_session(&stopped).unwrap();
    let corrupt = store.root().join("corrupt");
    fs::create_dir_all(&corrupt).unwrap();
    fs::write(corrupt.join("session.json"), "{").unwrap();

    let error = meet_service::status(&ctx, None).unwrap_err();
    assert!(
        matches!(&error, ComlinkError::MeetingSessionUnreadable { id, .. } if id == "corrupt"),
        "{error}"
    );
    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("session.json"));
    let error = meet_service::status(&ctx, Some("corrupt".to_string())).unwrap_err();
    assert!(matches!(
        error,
        ComlinkError::MeetingSessionUnreadable { .. }
    ));

    // The active pointer names the unreadable session: an error, not `none`.
    fs::write(store.root().join("active-session"), "corrupt").unwrap();
    assert!(matches!(
        meet_service::status(&ctx, None).unwrap_err(),
        ComlinkError::MeetingSessionUnreadable { .. }
    ));

    // A live transcribing session still takes precedence over the scan.
    fs::remove_file(store.root().join("active-session")).unwrap();
    let mut transcribing = constructed_session(&store, "busy");
    transcribing.status = MeetingStatus::Transcribing;
    store.save_session(&transcribing).unwrap();
    assert_eq!(
        meet_service::status(&ctx, None)
            .unwrap()
            .session_id
            .as_deref(),
        Some("busy")
    );
}

#[test]
fn status_reports_finalize_log_and_references_it_in_warnings() {
    let harness = ServiceHarness::new(MockOptions::default());
    let id = harness.transcribing_session();
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let session = harness.store().read_session(&id).unwrap();
    let log = meet_service::finalize_log_path(&session);

    // No log yet: null, and the stale reason does not point at a missing file.
    let report = meet_service::status(&ctx, Some(id.clone())).unwrap();
    assert!(report.stale);
    assert!(report.finalize_log.is_none());
    assert!(!report
        .stale_reason
        .as_deref()
        .unwrap()
        .contains("finalize.log"));
    let json = serde_json::to_value(&report).unwrap();
    assert!(json["finalize_log"].is_null());

    fs::write(&log, "").unwrap();
    let report = meet_service::status(&ctx, Some(id.clone())).unwrap();
    let log_text = log.display().to_string();
    assert_eq!(report.finalize_log.as_deref(), Some(log_text.as_str()));
    assert!(report
        .stale_reason
        .as_deref()
        .unwrap()
        .contains(&format!("see {log_text}")));
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains(&log_text)));

    let mut failed = harness.store().read_session(&id).unwrap();
    failed.mark_failed("mock failure");
    harness.store().save_session(&failed).unwrap();
    let report = meet_service::status(&ctx, Some(id.clone())).unwrap();
    assert_eq!(report.finalize_log.as_deref(), Some(log_text.as_str()));
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("finalize failed: mock failure")
            && warning.contains(&log_text)));
}

#[test]
fn process_launcher_reaps_the_finalizer_so_no_zombie_remains() {
    let harness = ServiceHarness::new(MockOptions::default());
    let store = harness.store();
    let session = constructed_session(&store, "reap");
    let marker = harness.root.join("finalizer-args");
    let mock = harness.root.join("mock-comlink");
    common::write_executable(
        &mock,
        &format!(
            "#!/bin/sh\nprintf '%s ' \"$@\" > \"{}.tmp\"\nmv \"{}.tmp\" \"{}\"\n",
            marker.display(),
            marker.display(),
            marker.display()
        ),
    );

    let launcher = meet_service::ProcessFinalizeLauncher::with_executable(&mock);
    let identity =
        meet_service::FinalizeLauncher::launch(&launcher, &session).expect("launch finalizer");
    let pid = identity.pid;
    assert_ne!(pid, std::process::id());

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "mock finalizer never ran"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read_to_string(&marker).unwrap().trim(),
        "meet finalize reap --format json"
    );
    assert!(meet_service::finalize_log_path(&session).is_file());

    // The child has exited. Unreaped it would stay in state `Z` for as long
    // as this (long-lived) process runs; reaped it disappears from `ps`.
    loop {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if state.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "finalizer pid {pid} was not reaped; ps state {state:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------
// LF-161 micro-round
// ---------------------------------------------------------------------------

#[test]
fn meeting_audio_audit_names_a_corrupt_session_that_still_holds_chunks() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let store = harness.store();
    fs::create_dir_all(store.root()).unwrap();
    assert!(meet_service::meeting_audio_audit(&ctx).clean);

    // No session.json and no chunks: not a session, nothing to report.
    fs::create_dir_all(store.root().join("empty-dir")).unwrap();
    // Corrupt session.json with no chunks: no audio, nothing to report.
    let no_audio = store.root().join("corrupt-no-audio");
    fs::create_dir_all(&no_audio).unwrap();
    fs::write(no_audio.join("session.json"), "{").unwrap();
    assert!(meet_service::meeting_audio_audit(&ctx).clean);

    // Corrupt session.json with chunk WAVs (one in a per-stream subdir).
    let corrupt = store.root().join("corrupt");
    fs::create_dir_all(corrupt.join("chunks/system")).unwrap();
    fs::write(corrupt.join("session.json"), "{").unwrap();
    fs::write(corrupt.join("chunks/chunk-00000.wav"), "leftover").unwrap();
    fs::write(corrupt.join("chunks/system/chunk-00000.wav"), "leftover").unwrap();

    let audit = meet_service::meeting_audio_audit(&ctx);
    assert!(!audit.clean);
    assert!(audit.scan_errors.is_empty());
    assert_eq!(audit.unretained_leftovers.len(), 1, "{audit:?}");
    let entry = &audit.unretained_leftovers[0];
    assert_eq!(entry.session_id, "corrupt");
    assert_eq!(entry.status, "unknown");
    assert_eq!(entry.chunk_files, 2);
    assert_eq!(entry.session_dir, corrupt.display().to_string());
    assert!(
        entry.reason.contains("unreadable session.json"),
        "{entry:?}"
    );
    assert!(entry.remedy.contains(&corrupt.display().to_string()));
}

#[test]
fn meeting_audio_audit_survives_an_unreadable_chunks_dir_and_names_it() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let store = harness.store();
    let id = harness.transcribing_session();
    meet_service::finalize(&ctx, &id, Duration::from_secs(1)).unwrap();
    let session = store.read_session(&id).unwrap();
    plant_leftover_chunks(&session);
    // A second, healthy leftover proves one bad dir does not hide the others.
    let other = constructed_session(&store, "other-failed");
    let mut other_failed = other.clone();
    other_failed.mark_failed("mock");
    store.save_session(&other_failed).unwrap();
    plant_leftover_chunks(&other);

    set_mode(&session.chunks_dir, 0o000);
    let audit = meet_service::meeting_audio_audit(&ctx);
    set_mode(&session.chunks_dir, 0o755);

    assert!(!audit.clean);
    assert_eq!(audit.unretained_leftovers.len(), 2, "{audit:?}");
    let entry = audit
        .unretained_leftovers
        .iter()
        .find(|entry| entry.session_id == id)
        .unwrap();
    assert_eq!(entry.status, "stopped");
    assert!(entry.reason.contains("unreadable chunks dir"), "{entry:?}");
    assert!(entry.reason.contains(&session.chunks_dir), "{entry:?}");
    assert_eq!(entry.chunks_dir, session.chunks_dir);
    let other_entry = audit
        .unretained_leftovers
        .iter()
        .find(|entry| entry.session_id == "other-failed")
        .unwrap();
    assert_eq!(other_entry.status, "failed");
    assert_eq!(other_entry.chunk_files, 2);
}

#[test]
fn meeting_audio_audit_flags_stale_recordings_but_not_live_ones() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let store = harness.store();

    // Live: the mock recorder is running and verified.
    let live = harness.start(&ctx, 2);
    let live_session = store.read_session(&live.session_id).unwrap();
    assert!(meet_service::session_recorder_is_verified_running(
        &live_session
    ));
    let audit = meet_service::meeting_audio_audit(&ctx);
    assert!(audit.clean, "live recording flagged: {audit:?}");
    meet_service::stop(&ctx, Some(live.session_id.clone()), STOP_WAIT).unwrap();

    // Stale: `recording` with chunks and no recorder running.
    let stale = constructed_session(&store, "stale-recording");
    assert_eq!(stale.status, MeetingStatus::Recording);
    let audit = meet_service::meeting_audio_audit(&ctx);
    assert!(
        audit.clean,
        "a stale recording without chunks holds no audio"
    );
    plant_leftover_chunks(&stale);
    let audit = meet_service::meeting_audio_audit(&ctx);
    assert!(!audit.clean);
    assert_eq!(audit.unretained_leftovers.len(), 1, "{audit:?}");
    let entry = &audit.unretained_leftovers[0];
    assert_eq!(entry.session_id, "stale-recording");
    assert_eq!(entry.status, "recording");
    assert_eq!(entry.chunk_files, 2);
    assert!(entry.reason.contains("stale recording"), "{entry:?}");
    assert_eq!(entry.remedy, "comlink meet stop stale-recording");
}

#[test]
fn meeting_audio_audit_records_an_unreadable_store_root_instead_of_failing() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let store = harness.store();
    // No store yet: clean.
    assert!(meet_service::meeting_audio_audit(&ctx).clean);
    fs::create_dir_all(store.root()).unwrap();
    let root = store.root().display().to_string();
    set_mode(&root, 0o000);
    let audit = meet_service::meeting_audio_audit(&ctx);
    set_mode(&root, 0o755);
    assert!(!audit.clean);
    assert_eq!(audit.scan_errors.len(), 1, "{audit:?}");
    assert_eq!(audit.scan_errors[0].path, root);
}

#[test]
fn sync_stop_cleanup_failure_is_failed_and_finalize_finishes_it() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let started = harness.start(&ctx, 2);
    let store = harness.store();

    // Read-only chunks dir: ASR can read the WAVs but they cannot be unlinked.
    set_mode(&started.chunks_dir, 0o555);
    let error = meet_service::stop(&ctx, None, STOP_WAIT).unwrap_err();
    set_mode(&started.chunks_dir, 0o755);
    assert!(
        matches!(&error, ComlinkError::MeetingChunkCleanupFailed { id, .. } if id == &started.session_id),
        "{error}"
    );
    assert_eq!(error.exit_code(), 1);
    // M3a: the reason names the chunks dir.
    assert!(error.to_string().contains(&started.chunks_dir), "{error}");

    let session = store.read_session(&started.session_id).unwrap();
    assert_eq!(session.status, MeetingStatus::Failed, "must not be stopped");
    assert!(
        session.error.as_deref().unwrap().contains("cleanup"),
        "{:?}",
        session.error
    );
    assert_eq!(session.chunks_processed, Some(2));
    assert_eq!(wav_count(&session.chunks_dir), 2, "audio is still on disk");
    assert!(store.validate_export_for_recovery(&session).is_ok());
    assert!(store.active_session_id().unwrap().is_none());
    assert!(!meet_service::meeting_audio_audit(&ctx).clean);

    let status = meet_service::finalize(&ctx, &started.session_id, Duration::from_secs(1)).unwrap();
    assert_eq!(status.status, "stopped");
    assert_eq!(status.chunks_processed, 2);
    assert_eq!(
        harness.whisper_invocations(),
        2,
        "finalize must not re-run ASR"
    );
    assert!(!Path::new(&session.chunks_dir).exists(), "audio is gone");
    let stopped = store.read_session(&started.session_id).unwrap();
    assert_eq!(stopped.status, MeetingStatus::Stopped);
    assert!(stopped.error.is_none());
    assert!(meet_service::meeting_audio_audit(&ctx).clean);
}

#[test]
fn launch_failure_logs_an_active_pointer_clear_failure() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(FailingLauncher));
    let started = harness.start(&ctx, 2);
    let store = harness.store();
    let root = store.root().display().to_string();

    // Read-only store root: the active-session pointer cannot be removed.
    set_mode(&root, 0o555);
    let error = meet_service::stop_detached(&ctx, None, STOP_WAIT).unwrap_err();
    set_mode(&root, 0o755);
    assert!(
        matches!(error, ComlinkError::MeetingFinalizeLaunchFailed(_)),
        "{error}"
    );
    let session = store.read_session(&started.session_id).unwrap();
    assert_eq!(session.status, MeetingStatus::Failed);
    let log = fs::read_to_string(meet_service::finalize_log_path(&session)).unwrap();
    assert!(
        log.contains("could not clear the active-session pointer")
            && log.contains(&started.session_id),
        "{log}"
    );
}

#[test]
fn finalize_never_overwrites_an_invalid_export_on_a_stopped_session() {
    let harness = ServiceHarness::new(MockOptions::default());
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let started = harness.start(&ctx, 2);
    meet_service::stop(&ctx, None, STOP_WAIT).unwrap();
    let store = harness.store();
    let session = store.read_session(&started.session_id).unwrap();
    assert_eq!(harness.whisper_invocations(), 2);

    // A user-edited (now invalid) export, with chunks back on disk.
    let edited = "{\"edited\": \"by the user\"}";
    fs::write(&session.json_export_path, edited).unwrap();
    plant_leftover_chunks(&session);

    let error =
        meet_service::finalize(&ctx, &started.session_id, Duration::from_secs(1)).unwrap_err();
    assert!(
        matches!(error, ComlinkError::MeetingExportUnavailable(_)),
        "{error}"
    );
    assert_eq!(harness.whisper_invocations(), 2, "ASR must not re-run");
    assert_eq!(
        fs::read_to_string(&session.json_export_path).unwrap(),
        edited
    );
    assert_eq!(
        store.read_session(&started.session_id).unwrap().status,
        MeetingStatus::Stopped
    );
    assert_eq!(wav_count(&session.chunks_dir), 2);
}
