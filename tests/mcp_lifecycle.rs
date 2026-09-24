#![cfg(unix)]
//! Meeting lifecycle driven entirely through the MCP server (in process, mock
//! ffmpeg/whisper): the `mcp.allow_start` gate, start -> status -> stop ->
//! poll -> get_transcript, warnings in results, and the tool error matrix.

mod common;

use std::{
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::Duration,
};

use comlink::{
    error::ComlinkError,
    mcp::McpContextFactory,
    meet_service::{self, MeetContext},
};
use common::{
    tool_err, tool_ok, CountingLauncher, FailingLauncher, McpClient, MockOptions, NoopLauncher,
    ServiceHarness, TestContextFactory,
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

#[tokio::test]
async fn system_only_and_mic_plus_system_sources_run_the_full_cycle() {
    for (source, labels, has_mic) in [
        ("system-only", vec!["system_audio"], false),
        ("mic-plus-system", vec!["user_mic", "system_audio"], true),
    ] {
        let harness = ServiceHarness::new(MockOptions::default());
        let launcher = harness.thread_launcher();
        let (mut client, _) =
            McpClient::initialized(harness.mcp_factory(launcher.clone(), true), PROTOCOL).await;

        let started = client
            .call_tool(
                "meeting_start",
                json!({
                    "source": source,
                    "mode": "raw",
                    "device": ":0",
                    "system_device": "BlackHole 2ch",
                    "no_llm": true
                }),
            )
            .await;
        let start = tool_ok(&started);
        assert_eq!(start["source"]["mode"], source, "{start:#}");
        let recorders = start["recorders"].as_array().unwrap();
        let recorder_labels: Vec<&str> = recorders
            .iter()
            .map(|recorder| recorder["source_label"].as_str().unwrap())
            .collect();
        assert_eq!(recorder_labels, labels, "{source}");
        // The system stream records the requested BlackHole input.
        let system_stream = start["source"]["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["label"] == "system_audio")
            .unwrap_or_else(|| panic!("{source}: no system_audio stream in {start:#}"));
        assert_eq!(system_stream["device"], ":1", "{source}");
        // System-only capture opens no microphone, so none is reported.
        assert_eq!(start["input_device"].is_null(), !has_mic, "{source}");
        if has_mic {
            assert_eq!(start["input_device"]["avfoundation_input"], ":0");
            assert_eq!(start["input_device"]["selected_by"], "--device");
        }
        for recorder in recorders {
            common::wait_for_chunks(
                std::path::Path::new(recorder["chunks_dir"].as_str().unwrap()),
                2,
            );
        }
        let id = start["session_id"].as_str().unwrap().to_string();

        let stop = tool_ok(&client.call_tool("meeting_stop", json!({})).await);
        assert_eq!(stop["status"], "transcribing");
        poll_status(&mut client, &id, "stopped").await;
        for result in launcher.join_all() {
            result.unwrap();
        }
        let transcript = tool_ok(
            &client
                .call_tool("meeting_get_transcript", json!({"format": "json"}))
                .await,
        );
        assert_eq!(transcript["content"]["source"]["mode"], source);
        common::assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
    }
}

#[tokio::test]
async fn start_without_a_device_reports_which_microphone_is_recorded() {
    let harness = ServiceHarness::new(MockOptions::default());
    let (mut client, _) =
        McpClient::initialized(harness.mcp_factory(Arc::new(NoopLauncher), true), PROTOCOL).await;
    let started = client
        .call_tool(
            "meeting_start",
            json!({"source": "mic-only", "mode": "raw", "no_llm": true}),
        )
        .await;
    let start = tool_ok(&started);
    let device = &start["input_device"];
    let input = device["avfoundation_input"].as_str().unwrap();
    // The recorder opens the device the agent is told about.
    assert_eq!(start["source"]["streams"][0]["device"], input);
    // Which default applies depends on the host (CoreAudio default input via
    // system_profiler, else the `:0` fallback) or an inherited
    // COMLINK_RECORD_DEVICE; each must be reported.
    let selected_by = device["selected_by"].as_str().unwrap();
    let expected_line = match selected_by {
        "system default input" => {
            let name = device["name"].as_str().unwrap();
            format!("Using system default input device: {name} ({input})")
        }
        "fallback" => format!("Using fallback input device {input}"),
        "COMLINK_RECORD_DEVICE" => "from COMLINK_RECORD_DEVICE".to_string(),
        other => panic!("unexpected selection {other}: {device}"),
    };
    // Second text block, right after the consent reminder.
    let line = started["content"][1]["text"].as_str().unwrap();
    assert!(
        line.contains(&expected_line),
        "{line:?} vs {expected_line:?}"
    );
    assert!(line.contains(input), "{line}");
    let id = start["session_id"].as_str().unwrap().to_string();
    common::wait_for_chunks(
        std::path::Path::new(start["chunks_dir"].as_str().unwrap()),
        2,
    );
    meet_service::stop(
        &harness.ctx(Arc::new(NoopLauncher)),
        Some(id),
        Duration::from_secs(5),
    )
    .unwrap();
}

/// Wraps the harness factory so a test can make the next context builds fail
/// (as `config::load` does for a malformed config.json) or panic.
struct FlakyFactory {
    inner: Arc<TestContextFactory>,
    mode: AtomicU8,
}

const FACTORY_OK: u8 = 0;
const FACTORY_CONFIG_ERROR: u8 = 1;
const FACTORY_PANIC: u8 = 2;

impl McpContextFactory for FlakyFactory {
    fn context(&self) -> Result<MeetContext, ComlinkError> {
        match self.mode.load(Ordering::SeqCst) {
            FACTORY_CONFIG_ERROR => Err(ComlinkError::ConfigParse {
                path: "/tmp/comlink-home/config.json".into(),
                source: serde_json::from_str::<Value>("{ bad").unwrap_err(),
            }),
            FACTORY_PANIC => panic!("factory exploded on purpose"),
            _ => self.inner.context(),
        }
    }
}

#[tokio::test]
async fn a_failing_or_panicking_context_is_a_tool_error_and_the_server_keeps_serving() {
    let harness = ServiceHarness::new(MockOptions::default());
    let factory = Arc::new(FlakyFactory {
        inner: harness.mcp_factory(Arc::new(NoopLauncher), true),
        mode: AtomicU8::new(FACTORY_OK),
    });
    let (mut client, _) = McpClient::initialized(factory.clone(), PROTOCOL).await;
    tool_ok(&client.call_tool("meeting_status", json!({})).await);

    // A malformed config mid-session: every tool is isError config_parse.
    factory.mode.store(FACTORY_CONFIG_ERROR, Ordering::SeqCst);
    for (tool, arguments) in [
        ("meeting_start", start_args()),
        ("meeting_status", json!({})),
        ("meeting_stop", json!({})),
        ("meeting_get_transcript", json!({"format": "md"})),
        ("meeting_list", json!({})),
    ] {
        let message = tool_err(&client.call_tool(tool, arguments).await, "config_parse");
        assert!(message.contains("config.json"), "{tool}: {message}");
    }
    // Resources cannot carry isError: JSON-RPC internal error with the code.
    for (method, params) in [
        ("resources/list", json!({})),
        (
            "resources/read",
            json!({"uri": "comlink://meetings/m1/transcript.md"}),
        ),
    ] {
        let response = client.request(method, params).await;
        assert_eq!(response["error"]["code"], -32603, "{method}: {response:#}");
        assert_eq!(response["error"]["data"]["error_code"], "config_parse");
    }

    // A panic inside a service call is internal_panic with the panic text.
    factory.mode.store(FACTORY_PANIC, Ordering::SeqCst);
    let message = tool_err(
        &client.call_tool("meeting_status", json!({})).await,
        comlink::mcp::INTERNAL_PANIC_CODE,
    );
    assert!(message.contains("factory exploded on purpose"), "{message}");
    let response = client.request("resources/list", json!({})).await;
    assert_eq!(
        response["error"]["data"]["error_code"],
        comlink::mcp::INTERNAL_PANIC_CODE
    );

    // Fixed config: the same server answers normally again.
    factory.mode.store(FACTORY_OK, Ordering::SeqCst);
    assert_eq!(
        tool_ok(&client.call_tool("meeting_status", json!({})).await)["status"],
        "none"
    );
    let list = client.request("resources/list", json!({})).await;
    assert_eq!(list["result"]["resources"], json!([]));
    common::assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
}
