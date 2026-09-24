#![cfg(unix)]
//! Meeting lifecycle driven entirely through the MCP server (in process, mock
//! ffmpeg/whisper): the `mcp.allow_start` gate, start -> status -> stop ->
//! poll -> get_transcript, warnings in results, and the tool error matrix.

mod common;

use std::{sync::Arc, time::Duration};

use comlink::meet_service;
use common::{
    tool_err, tool_ok, CountingLauncher, FailingLauncher, McpClient, MockOptions, NoopLauncher,
    ServiceHarness,
};
use serde_json::{json, Value};

const PROTOCOL: &str = "2025-06-18";

fn start_args() -> Value {
    json!({"source": "mic-only", "mode": "raw", "device": ":0", "no_llm": true})
}

/// Poll `meeting_status` for `id` until it reports `want`, with a generous
/// bound (the finalizer runs on another thread).
async fn poll_status(client: &mut McpClient, id: &str, want: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..600 {
        last = tool_ok(
            &client
                .call_tool("meeting_status", json!({ "id": id }))
                .await,
        );
        if last["status"] == want {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {want}; last status {last:#}");
}

#[tokio::test]
async fn start_is_refused_until_allow_start_and_other_tools_still_work() {
    let harness = ServiceHarness::new(MockOptions::default());
    let factory = harness.mcp_factory(Arc::new(NoopLauncher), false);
    let (mut client, _) = McpClient::initialized(factory.clone(), PROTOCOL).await;

    let refused = client.call_tool("meeting_start", start_args()).await;
    let message = tool_err(&refused, "mcp_start_disabled");
    assert!(message.contains("comlink config set mcp.allow_start true"));
    assert!(harness.store().list_sessions().unwrap().0.is_empty());
    assert!(harness.store().active_session_id().unwrap().is_none());

    let status = tool_ok(&client.call_tool("meeting_status", json!({})).await);
    assert_eq!(status["status"], "none");
    let list = tool_ok(&client.call_tool("meeting_list", json!({})).await);
    assert_eq!(list["sessions"], json!([]));
    tool_err(
        &client.call_tool("meeting_stop", json!({})).await,
        "meeting_no_active_session",
    );
    tool_err(
        &client
            .call_tool("meeting_get_transcript", json!({"format": "md"}))
            .await,
        "meeting_no_active_session",
    );

    // Opting in takes effect on the next call, without a server restart.
    factory.set_allow_start(true);
    let started = client.call_tool("meeting_start", start_args()).await;
    let status = tool_ok(&started);
    assert_eq!(status["status"], "recording");
    let id = status["session_id"].as_str().unwrap().to_string();
    common::wait_for_chunks(
        std::path::Path::new(status["chunks_dir"].as_str().unwrap()),
        2,
    );
    meet_service::stop(
        &harness.ctx(Arc::new(NoopLauncher)),
        Some(id),
        Duration::from_secs(5),
    )
    .unwrap();
}

#[tokio::test]
async fn full_cycle_start_status_stop_poll_and_transcripts() {
    let harness = ServiceHarness::new(MockOptions::default());
    let launcher = harness.thread_launcher();
    let (mut client, _) =
        McpClient::initialized(harness.mcp_factory(launcher.clone(), true), PROTOCOL).await;

    let started = client.call_tool("meeting_start", start_args()).await;
    let start = tool_ok(&started);
    assert_eq!(start["schema_version"], "comlink.meeting.v1");
    assert_eq!(start["status"], "recording");
    assert_eq!(start["consent_reminder"], meet_service::CONSENT_REMINDER);
    // The consent reminder is the first thing the agent reads.
    assert_eq!(
        started["content"][0]["text"],
        meet_service::CONSENT_REMINDER
    );
    let id = start["session_id"].as_str().unwrap().to_string();
    common::wait_for_chunks(
        std::path::Path::new(start["chunks_dir"].as_str().unwrap()),
        2,
    );

    let status = tool_ok(&client.call_tool("meeting_status", json!({})).await);
    assert_eq!(status["session_id"], id);
    assert_eq!(status["status"], "recording");
    // Every MeetStatusReport field reaches the agent.
    for key in [
        "stale",
        "stale_reason",
        "error",
        "finalize_log",
        "warnings",
        "audio_level",
        "recorders",
        "finalizer",
        "chunk_count",
    ] {
        assert!(
            status.get(key).is_some(),
            "status is missing {key}: {status}"
        );
    }
    assert_eq!(status["stale"], false);
    assert_eq!(status["recorders"][0]["alive"], true);

    let stopped = client.call_tool("meeting_stop", json!({})).await;
    let stop = tool_ok(&stopped);
    assert_eq!(stop["status"], "transcribing");
    assert_eq!(stop["session_id"], id);
    assert!(stopped["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Poll meeting_status"));

    let final_status = poll_status(&mut client, &id, "stopped").await;
    assert_eq!(final_status["stale"], false);
    for result in launcher.join_all() {
        result.unwrap();
    }

    let markdown = client
        .call_tool("meeting_get_transcript", json!({"format": "md"}))
        .await;
    let transcript = tool_ok(&markdown);
    assert_eq!(transcript["session_id"], id);
    assert_eq!(transcript["status"], "stopped");
    assert_eq!(transcript["format"], "md");
    assert_eq!(transcript["transcript_retained"], true);
    let content = transcript["content"].as_str().unwrap();
    assert!(content.contains("Meeting segment 00000"), "{content}");
    // The text block is the Markdown itself.
    let last_text = markdown["content"].as_array().unwrap().last().unwrap()["text"].clone();
    assert_eq!(last_text, transcript["content"]);

    let json_result = client
        .call_tool(
            "meeting_get_transcript",
            json!({"id": id, "format": "json"}),
        )
        .await;
    let transcript = tool_ok(&json_result);
    assert_eq!(transcript["format"], "json");
    let export = &transcript["content"];
    assert!(export.is_object(), "json content is an object: {export}");
    assert_eq!(export["schema_version"], "comlink.meeting.v1");
    assert_eq!(export["session"]["session_id"], id);
    assert_eq!(export["segments"].as_array().unwrap().len(), 2);

    let list = tool_ok(&client.call_tool("meeting_list", json!({})).await);
    assert_eq!(list["sessions"][0]["session_id"], id);
    assert_eq!(list["sessions"][0]["status"], "stopped");
    common::assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
}

#[tokio::test]
async fn near_silent_capture_warning_reaches_status_and_transcript() {
    let harness = ServiceHarness::new(MockOptions {
        silent: true,
        ..MockOptions::default()
    });
    let launcher = harness.thread_launcher();
    let (mut client, _) =
        McpClient::initialized(harness.mcp_factory(launcher.clone(), true), PROTOCOL).await;
    let start = tool_ok(&client.call_tool("meeting_start", start_args()).await);
    let id = start["session_id"].as_str().unwrap().to_string();
    common::wait_for_chunks(
        std::path::Path::new(start["chunks_dir"].as_str().unwrap()),
        2,
    );
    tool_ok(&client.call_tool("meeting_stop", json!({})).await);
    let status = poll_status(&mut client, &id, "stopped").await;
    for result in launcher.join_all() {
        result.unwrap();
    }

    let has_near_silent = |value: &Value| {
        value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("near-silent"))
    };
    assert!(has_near_silent(&status), "status warnings: {status:#}");
    assert_eq!(status["audio_level"]["near_silent"], true);

    let result = client
        .call_tool("meeting_get_transcript", json!({"format": "md"}))
        .await;
    let transcript = tool_ok(&result);
    assert!(has_near_silent(&transcript), "{transcript:#}");
    assert_eq!(transcript["audio_level"]["near_silent"], true);
    // And as a readable text line, not only inside the JSON.
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("warning: "));
}

#[tokio::test]
async fn tool_error_matrix_is_is_error_with_stable_codes() {
    let harness = ServiceHarness::new(MockOptions::default());
    let counting = Arc::new(CountingLauncher::default());
    let factory = harness.mcp_factory(counting.clone(), true);
    let (mut client, _) = McpClient::initialized(factory.clone(), PROTOCOL).await;

    // none
    tool_err(
        &client
            .call_tool("meeting_get_transcript", json!({"format": "json"}))
            .await,
        "meeting_no_active_session",
    );
    tool_err(
        &client.call_tool("meeting_stop", json!({})).await,
        "meeting_no_active_session",
    );
    // unknown id
    for tool in ["meeting_status", "meeting_stop"] {
        tool_err(
            &client
                .call_tool(tool, json!({"id": "no-such-meeting"}))
                .await,
            "meeting_session_not_found",
        );
    }
    tool_err(
        &client
            .call_tool(
                "meeting_get_transcript",
                json!({"id": "no-such-meeting", "format": "md"}),
            )
            .await,
        "meeting_session_not_found",
    );
    // invalid mode
    tool_err(
        &client
            .call_tool(
                "meeting_start",
                json!({"source": "mic-only", "mode": "no-such-mode", "device": ":0"}),
            )
            .await,
        "mode_not_found",
    );

    // already active
    let start = tool_ok(&client.call_tool("meeting_start", start_args()).await);
    let recording = start["session_id"].as_str().unwrap().to_string();
    common::wait_for_chunks(
        std::path::Path::new(start["chunks_dir"].as_str().unwrap()),
        2,
    );
    let message = tool_err(
        &client.call_tool("meeting_start", start_args()).await,
        "meeting_already_active",
    );
    assert!(message.contains(&recording));
    // A recording session has no transcript yet.
    tool_err(
        &client
            .call_tool(
                "meeting_get_transcript",
                json!({"id": recording, "format": "md"}),
            )
            .await,
        "meeting_not_stopped",
    );

    // still transcribing: the counting launcher starts no finalizer.
    tool_ok(&client.call_tool("meeting_stop", json!({})).await);
    assert_eq!(counting.count(), 1, "meeting_stop takes the detached path");
    for arguments in [
        json!({"format": "md"}),
        json!({"id": recording, "format": "json"}),
    ] {
        let message = tool_err(
            &client.call_tool("meeting_get_transcript", arguments).await,
            "meeting_still_transcribing",
        );
        assert!(message.contains(&recording));
    }
    // Stopping it again is not a crash either.
    tool_err(
        &client
            .call_tool("meeting_stop", json!({"id": recording}))
            .await,
        "meeting_not_recording",
    );

    // failed: the finalizer cannot be launched.
    let failing_ctx = harness.ctx(Arc::new(FailingLauncher));
    let failed = harness.start(&failing_ctx, 2).session_id;
    meet_service::stop_detached(&failing_ctx, None, Duration::from_secs(5)).unwrap_err();
    for arguments in [
        json!({"format": "md"}),
        json!({"id": failed, "format": "md"}),
    ] {
        let message = tool_err(
            &client.call_tool("meeting_get_transcript", arguments).await,
            "meeting_finalize_failed",
        );
        assert!(message.contains("mock spawn failure"), "{message}");
        assert!(
            message.contains(&format!("comlink meet finalize {failed}")),
            "{message}"
        );
    }
    let status = tool_ok(
        &client
            .call_tool("meeting_status", json!({"id": failed}))
            .await,
    );
    assert_eq!(status["status"], "failed");
    assert!(status["error"]
        .as_str()
        .unwrap()
        .contains("mock spawn failure"));
    common::assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
}

#[tokio::test]
async fn retention_off_transcript_succeeds_with_transcript_retained_false() {
    let harness = ServiceHarness::new(MockOptions::default());
    let factory = harness.mcp_factory(Arc::new(NoopLauncher), true);
    factory.set_retain_transcripts(false);
    let (mut client, _) = McpClient::initialized(factory.clone(), PROTOCOL).await;
    let start = tool_ok(&client.call_tool("meeting_start", start_args()).await);
    let id = start["session_id"].as_str().unwrap().to_string();
    common::wait_for_chunks(
        std::path::Path::new(start["chunks_dir"].as_str().unwrap()),
        2,
    );
    // Finish synchronously in the same (retention-off) config.
    let mut ctx = harness.ctx(Arc::new(NoopLauncher));
    ctx.resolved.config.retention.transcripts = false;
    meet_service::stop(&ctx, None, Duration::from_secs(5)).unwrap();

    let result = client
        .call_tool(
            "meeting_get_transcript",
            json!({"id": id, "format": "json"}),
        )
        .await;
    let transcript = tool_ok(&result);
    assert_eq!(transcript["transcript_retained"], false);
    assert_eq!(transcript["content"]["retention"]["transcripts"], false);
    assert!(transcript["content"]["final_text"].is_null());
    assert!(result["content"]
        .as_array()
        .unwrap()
        .iter()
        .any(|block| block["text"]
            .as_str()
            .unwrap()
            .contains("retention.transcripts is off")));
}
