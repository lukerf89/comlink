#![cfg(unix)]
//! Wire-level tests for the `comlink mcp` server: an in-process `ComlinkMcp`
//! driven by raw JSON-RPC lines over an in-memory duplex pipe, so the exact
//! frames (protocol version, schemas, annotations, error codes) are asserted.

mod common;

use std::{fs, sync::Arc, time::Duration};

use comlink::{mcp, meet_service};
use common::{
    assert_json_rpc_lines, FailingLauncher, McpClient, MockOptions, NoopLauncher, ServiceHarness,
};
use serde_json::{json, Value};

const STOP_WAIT: Duration = Duration::from_secs(5);

fn harness() -> ServiceHarness {
    ServiceHarness::new(MockOptions::default())
}

/// A stopped session with exports, via the synchronous service stop.
fn stopped_session(harness: &ServiceHarness) -> String {
    let ctx = harness.ctx(Arc::new(NoopLauncher));
    let started = harness.start(&ctx, 2);
    meet_service::stop(&ctx, None, STOP_WAIT).unwrap();
    started.session_id
}

#[tokio::test]
async fn initialize_negotiates_only_audited_protocol_versions() {
    let harness = harness();
    for (requested, expected) in [
        ("2025-03-26", "2025-03-26"),
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-11-25"),
        // Not audited: answered with our latest, per the spec.
        ("2024-11-05", "2025-11-25"),
        ("2026-07-28", "2025-11-25"),
        ("2099-01-01", "2025-11-25"),
    ] {
        let factory = harness.mcp_factory(Arc::new(NoopLauncher), false);
        let (client, response) = McpClient::initialized(factory, requested).await;
        let result = &response["result"];
        assert_eq!(result["protocolVersion"], expected, "requested {requested}");
        assert_eq!(result["serverInfo"]["name"], "comlink");
        assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["capabilities"]["resources"].is_object());
        let instructions = result["instructions"].as_str().unwrap();
        assert!(instructions.contains("mcp.allow_start"));
        assert!(instructions.contains("sent to you, the calling model"));
        assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
    }
    assert_eq!(
        mcp::SUPPORTED_PROTOCOL_VERSIONS
            .iter()
            .map(|version| version.as_str())
            .collect::<Vec<_>>(),
        ["2025-03-26", "2025-06-18", "2025-11-25"]
    );
}

#[tokio::test]
async fn tools_list_has_exact_names_schemas_and_every_annotation() {
    let harness = harness();
    let (mut client, _) = McpClient::initialized(
        harness.mcp_factory(Arc::new(NoopLauncher), false),
        "2025-06-18",
    )
    .await;
    let response = client.request("tools/list", json!({})).await;
    let tools = response["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "meeting_get_transcript",
            "meeting_list",
            "meeting_start",
            "meeting_status",
            "meeting_stop"
        ]
    );
    let tool = |name: &str| -> Value {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .cloned()
            .unwrap()
    };

    // name -> (readOnly, destructive, idempotent); openWorld is always false.
    for (name, read_only, destructive, idempotent) in [
        ("meeting_start", false, false, false),
        ("meeting_stop", false, true, false),
        ("meeting_status", true, false, true),
        ("meeting_get_transcript", true, false, true),
        ("meeting_list", true, false, true),
    ] {
        let annotations = &tool(name)["annotations"];
        assert_eq!(annotations["readOnlyHint"], read_only, "{name}");
        assert_eq!(annotations["destructiveHint"], destructive, "{name}");
        assert_eq!(annotations["idempotentHint"], idempotent, "{name}");
        assert_eq!(annotations["openWorldHint"], false, "{name}");
        assert!(annotations["title"].is_string(), "{name}");
        assert!(tool(name)["description"].as_str().unwrap().len() > 20);
        assert_eq!(tool(name)["inputSchema"]["type"], "object", "{name}");
        assert!(tool(name).get("outputSchema").is_none(), "{name}");
    }

    let start = tool("meeting_start");
    let schema = &start["inputSchema"];
    let mut required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    required.sort();
    assert_eq!(required, ["mode", "source"]);
    for optional in ["device", "system_device", "no_llm"] {
        assert!(schema["properties"][optional].is_object(), "{optional}");
    }
    let source_values = enum_values(schema, "source");
    assert_eq!(
        source_values,
        ["mic-only", "system-only", "mic-plus-system"]
    );

    let transcript = tool("meeting_get_transcript");
    let schema = &transcript["inputSchema"];
    assert_eq!(schema["required"], json!(["format"]));
    assert_eq!(enum_values(schema, "format"), ["md", "json"]);
    assert!(schema["properties"]["id"].is_object());

    for name in ["meeting_status", "meeting_stop"] {
        let schema = &tool(name)["inputSchema"];
        assert!(schema["properties"]["id"].is_object(), "{name}");
        assert!(
            schema.get("required").is_none() || schema["required"] == json!([]),
            "{name}: id is optional"
        );
    }
    assert!(tool("meeting_stop")["description"]
        .as_str()
        .unwrap()
        .contains("Poll meeting_status"));
    assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
}

/// The string values a property accepts, whether schemars emitted `enum` or
/// a `oneOf` of `const`s behind a `$ref`.
fn enum_values(schema: &Value, property: &str) -> Vec<String> {
    let mut node = schema["properties"][property].clone();
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let name = reference.rsplit('/').next().unwrap();
        node = schema["$defs"][name].clone();
    }
    if let Some(values) = node.get("enum").and_then(Value::as_array) {
        return values
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect();
    }
    node["oneOf"]
        .as_array()
        .unwrap_or_else(|| panic!("{property} has no enum: {node}"))
        .iter()
        .map(|variant| variant["const"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn resource_templates_and_list_expose_both_transcript_formats() {
    let harness = harness();
    let id = stopped_session(&harness);
    // A second, still-transcribing session must not be listed.
    harness.transcribing_session();
    let (mut client, _) = McpClient::initialized(
        harness.mcp_factory(Arc::new(NoopLauncher), false),
        "2025-11-25",
    )
    .await;

    let templates = client.request("resources/templates/list", json!({})).await;
    let templates = templates["result"]["resourceTemplates"].as_array().unwrap();
    let pairs: Vec<(String, String)> = templates
        .iter()
        .map(|template| {
            (
                template["uriTemplate"].as_str().unwrap().to_string(),
                template["mimeType"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        pairs,
        [
            (
                "comlink://meetings/{id}/transcript.md".to_string(),
                "text/markdown".to_string()
            ),
            (
                "comlink://meetings/{id}/transcript.json".to_string(),
                "application/json".to_string()
            ),
        ]
    );

    let listed = client.request("resources/list", json!({})).await;
    let uris: Vec<&str> = listed["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|resource| resource["uri"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        [
            format!("comlink://meetings/{id}/transcript.md"),
            format!("comlink://meetings/{id}/transcript.json"),
        ]
    );
    assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
}

#[tokio::test]
async fn resources_read_returns_markdown_and_json_exports() {
    let harness = harness();
    let id = stopped_session(&harness);
    let (mut client, _) = McpClient::initialized(
        harness.mcp_factory(Arc::new(NoopLauncher), false),
        "2025-06-18",
    )
    .await;

    let uri = format!("comlink://meetings/{id}/transcript.md");
    let response = client
        .request("resources/read", json!({ "uri": uri }))
        .await;
    let contents = &response["result"]["contents"][0];
    assert_eq!(contents["uri"], uri);
    assert_eq!(contents["mimeType"], "text/markdown");
    let markdown = contents["text"].as_str().unwrap();
    let expected = fs::read_to_string(
        harness
            .store()
            .read_session(&id)
            .unwrap()
            .markdown_export_path,
    )
    .unwrap();
    assert_eq!(markdown, expected);
    assert!(markdown.contains("Meeting segment"));

    let uri = format!("comlink://meetings/{id}/transcript.json");
    let response = client
        .request("resources/read", json!({ "uri": uri }))
        .await;
    let contents = &response["result"]["contents"][0];
    assert_eq!(contents["mimeType"], "application/json");
    let export: Value = serde_json::from_str(contents["text"].as_str().unwrap()).unwrap();
    assert_eq!(export["schema_version"], "comlink.meeting.v1");
    assert_eq!(export["session"]["session_id"], id);
    assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
}

#[tokio::test]
async fn resources_read_error_matrix_uses_stable_codes() {
    let harness = harness();
    let stopped = stopped_session(&harness);
    let transcribing = harness.transcribing_session();
    // A failed session: the finalizer could not be launched.
    let failed = {
        let ctx = harness.ctx(Arc::new(FailingLauncher));
        let started = harness.start(&ctx, 2);
        meet_service::stop_detached(&ctx, None, STOP_WAIT).unwrap_err();
        started.session_id
    };
    let missing_export = {
        let id = stopped_session(&harness);
        let session = harness.store().read_session(&id).unwrap();
        fs::remove_file(&session.json_export_path).unwrap();
        id
    };
    let (mut client, _) = McpClient::initialized(
        harness.mcp_factory(Arc::new(NoopLauncher), false),
        "2025-06-18",
    )
    .await;

    let cases: Vec<(String, i64, &str, Option<String>)> = vec![
        (
            "file:///etc/passwd".into(),
            -32602,
            "invalid_resource_uri",
            None,
        ),
        (
            format!("comlink://meetings/{stopped}/transcript.txt"),
            -32602,
            "invalid_resource_uri",
            None,
        ),
        (
            "comlink://meetings/..%2F..%2Fetc/transcript.md".into(),
            -32602,
            "invalid_resource_uri",
            None,
        ),
        (
            format!("comlink://meetings/{stopped}/extra/transcript.md"),
            -32602,
            "invalid_resource_uri",
            None,
        ),
        (
            "comlink://meetings/no-such-meeting/transcript.md".into(),
            -32002,
            "meeting_session_not_found",
            None,
        ),
        (
            format!("comlink://meetings/{transcribing}/transcript.md"),
            -32600,
            "meeting_still_transcribing",
            Some(format!("comlink meet status {transcribing}")),
        ),
        (
            format!("comlink://meetings/{failed}/transcript.json"),
            -32600,
            "meeting_finalize_failed",
            Some(format!("comlink meet finalize {failed}")),
        ),
        (
            format!("comlink://meetings/{missing_export}/transcript.md"),
            -32600,
            "meeting_export_unavailable",
            None,
        ),
    ];
    for (uri, code, error_code, needle) in cases {
        let response = client
            .request("resources/read", json!({ "uri": uri }))
            .await;
        let error = &response["error"];
        assert!(response.get("result").is_none(), "{uri}: {response}");
        assert_eq!(error["code"], code, "{uri}: {response}");
        assert_eq!(error["data"]["error_code"], error_code, "{uri}");
        assert_eq!(error["data"]["message"], error["message"], "{uri}");
        if let Some(needle) = needle {
            assert!(
                error["message"].as_str().unwrap().contains(&needle),
                "{uri}: {error}"
            );
        }
    }
    // The failed session's message carries the recorded error text.
    let response = client
        .request(
            "resources/read",
            json!({ "uri": format!("comlink://meetings/{failed}/transcript.md") }),
        )
        .await;
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("mock spawn failure"));

    // The server is still healthy after every error.
    let status = client.call_tool("meeting_list", json!({})).await;
    assert_eq!(status["isError"], false);
    assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
}

#[tokio::test]
async fn bad_tool_arguments_never_reach_the_service_and_unsafe_ids_are_not_found() {
    let harness = harness();
    let (mut client, _) = McpClient::initialized(
        harness.mcp_factory(Arc::new(NoopLauncher), true),
        "2025-06-18",
    )
    .await;
    // Schema violations never reach the service: rmcp reports them as a tool
    // error naming the bad value, without structuredContent.
    for (name, arguments, needle) in [
        (
            "meeting_start",
            json!({"source": "radio", "mode": "raw"}),
            "unknown variant `radio`",
        ),
        ("meeting_start", json!({"source": "mic-only"}), "mode"),
        ("meeting_get_transcript", json!({"format": "pdf"}), "`pdf`"),
    ] {
        let result = client.call_tool(name, arguments).await;
        assert_eq!(result["isError"], true, "{result}");
        assert!(result.get("structuredContent").is_none(), "{result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("failed to deserialize parameters"), "{text}");
        assert!(text.contains(needle), "{text}");
    }
    assert!(harness.store().list_sessions().unwrap().0.is_empty());
    let response = client
        .request(
            "tools/call",
            json!({"name": "meeting_delete", "arguments": {}}),
        )
        .await;
    assert!(
        response["error"].is_object() || response["result"]["isError"] == true,
        "{response}"
    );

    // Ids that could escape the store read as not found.
    for id in ["../../etc", "a/b", ""] {
        let result = client
            .call_tool("meeting_status", json!({ "id": id }))
            .await;
        common::tool_err(&result, "meeting_session_not_found");
    }
    assert_json_rpc_lines(client.server_lines.iter().map(String::as_str));
}
