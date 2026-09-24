//! Proves the meeting service writes nothing to stdout (the future MCP stdio
//! transport) or stderr. `harness = false`: `main` re-executes this binary as
//! a child with stdout/stderr piped, the child drives every service call
//! against a mock runtime, and the parent asserts zero bytes were written.
//! No process-global fd redirection is involved, so nothing can interfere with
//! other tests.

#[cfg(unix)]
mod common;

#[cfg(unix)]
const CHILD_ENV: &str = "COMLINK_STDOUT_PROBE_CHILD";

#[cfg(unix)]
fn main() {
    if std::env::var_os(CHILD_ENV).is_some() {
        run_child();
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .env(CHILD_ENV, "1")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout probe child failed: {:?}",
        output.status
    );
    assert_eq!(
        output.stdout.len(),
        0,
        "meeting service wrote to stdout: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        output.stderr.len(),
        0,
        "meeting service wrote to stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("meet_service_stdout: service calls wrote 0 bytes to stdout and stderr");
}

#[cfg(unix)]
fn run_child() {
    use std::time::Duration;

    use comlink::{meet, meet_service};
    use common::{MockOptions, ServiceHarness};

    let harness = ServiceHarness::new(MockOptions::default());
    let launcher = harness.thread_launcher();
    let ctx = harness.ctx(launcher.clone());

    let started = harness.start(&ctx, 2);
    let status = meet_service::status(&ctx, None).unwrap();
    assert_eq!(status.status, "recording");
    meet_service::stop_detached(&ctx, None, Duration::from_secs(5)).unwrap();
    for result in launcher.join_all() {
        result.unwrap();
    }
    let finalized =
        meet_service::finalize(&ctx, &started.session_id, Duration::from_secs(5)).unwrap();
    assert_eq!(finalized.status, "stopped");
    meet_service::export(
        &ctx,
        Some(started.session_id.clone()),
        meet::MeetingExportKind::Json,
    )
    .unwrap();
    meet_service::export(&ctx, None, meet::MeetingExportKind::Markdown).unwrap();
    let listed = meet_service::list(&ctx).unwrap();
    assert_eq!(listed.sessions.len(), 1);

    // And the synchronous path.
    harness.start(&ctx, 2);
    meet_service::stop(&ctx, None, Duration::from_secs(5)).unwrap();
    meet_service::status(&ctx, None).unwrap();
}

#[cfg(not(unix))]
fn main() {}
