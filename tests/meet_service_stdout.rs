//! Proves the meeting service, and the MCP adapter over it, write nothing to
//! stdout (the MCP stdio transport) or stderr. `harness = false`: `main`
//! re-executes this binary as a child with stdout/stderr piped, the child
//! drives every service call (and an MCP tool cycle) against a mock runtime,
//! and the parent asserts zero bytes were written.
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

    // LF-162 additions used by the MCP server.
    let prepared = meet_service::prepare_start(
        &harness.resolved,
        Some(harness.runtime.clone()),
        meet_service::StartOptions {
            mode: "raw".to_string(),
            source: "mic-only".to_string(),
            device: Some(":0".to_string()),
            system_device: None,
            chunk_seconds: 30,
            no_llm: true,
        },
    )
    .unwrap();
    let (_ctx, _request, note) = prepared.into_context(harness.resolved.clone());
    assert!(note.is_none());
    meet_service::transcript(&ctx, None, meet::MeetingExportKind::Markdown).unwrap();
    meet_service::transcript(
        &ctx,
        Some(started.session_id.clone()),
        meet::MeetingExportKind::Json,
    )
    .unwrap();
    meet_service::mcp_privacy(&harness.resolved);

    // The MCP adapter itself: a full tool cycle over an in-memory transport
    // must not write a byte to this process's stdout or stderr either.
    run_mcp_cycle(&harness);
}

#[cfg(unix)]
fn run_mcp_cycle(harness: &common::ServiceHarness) {
    use common::{tool_ok, McpClient};
    use serde_json::json;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let launcher = harness.thread_launcher();
    let factory = harness.mcp_factory(launcher.clone(), true);
    runtime.block_on(async {
        let (mut client, _) = McpClient::initialized(factory, "2025-06-18").await;
        client.request("tools/list", json!({})).await;
        let start = tool_ok(
            &client
                .call_tool(
                    "meeting_start",
                    json!({"source": "mic-only", "mode": "raw", "device": ":0"}),
                )
                .await,
        );
        let id = start["session_id"].as_str().unwrap().to_string();
        common::wait_for_chunks(
            std::path::Path::new(start["chunks_dir"].as_str().unwrap()),
            2,
        );
        tool_ok(&client.call_tool("meeting_status", json!({})).await);
        tool_ok(&client.call_tool("meeting_stop", json!({})).await);
        loop {
            let status = tool_ok(&client.call_tool("meeting_status", json!({"id": id})).await);
            if status["status"] == "stopped" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        tool_ok(
            &client
                .call_tool("meeting_get_transcript", json!({"format": "md"}))
                .await,
        );
        client
            .request(
                "resources/read",
                json!({"uri": format!("comlink://meetings/{id}/transcript.json")}),
            )
            .await;
        client.request("resources/list", json!({})).await;
        // An error path too.
        client
            .call_tool("meeting_stop", json!({"id": "no-such-meeting"}))
            .await;
    });
    for result in launcher.join_all() {
        result.unwrap();
    }
}

#[cfg(not(unix))]
fn main() {}
