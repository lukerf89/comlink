//! Local stdio MCP server (`comlink mcp`).
//!
//! A thin adapter over [`crate::meet_service`]: every tool and resource maps
//! its arguments onto one service call, runs it on tokio's blocking pool, and
//! maps the typed result (or [`ComlinkError`]) onto MCP. No meeting lifecycle
//! logic lives here and the server keeps no session state: everything is in
//! the `FileMeetingStore`, so the CLI and MCP act on the same sessions and a
//! recording survives the server or client restarting.
//!
//! stdout carries only JSON-RPC frames written by rmcp. Nothing here prints,
//! no tracing subscriber is installed, and transcript text is never logged.

use std::{borrow::Cow, sync::Arc};

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ContentBlock, Implementation, ListResourceTemplatesResult,
        ListResourcesResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ResourceTemplate,
        ServerCapabilities, ServerConfig,
    },
    schemars::JsonSchema,
    service::RequestContext,
    tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    config::{self, CliConfigOverrides},
    error::ComlinkError,
    meet,
    meet_service::{self, MeetContext},
    record::{DeviceSource, ResolvedRecordDevice},
};

/// MCP protocol revisions this server has been checked against (the
/// `initialize`-handshake revisions rmcp 3.4.1 knows, from 2025-03-26 on). A
/// client asking for any other version is answered with
/// [`LATEST_PROTOCOL_VERSION`], as the spec requires.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
];
pub const LATEST_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2025_11_25;

/// `structuredContent.error_code` for a tool whose service call panicked.
pub const INTERNAL_PANIC_CODE: &str = "internal_panic";
/// `structuredContent.error_code` for a service call that was cancelled
/// before it finished (e.g. the runtime shutting down).
pub const INTERNAL_CANCELLED_CODE: &str = "internal_cancelled";
/// `data.error_code` for a resource URI that is not one of the templates.
pub const INVALID_RESOURCE_URI_CODE: &str = "invalid_resource_uri";

pub const RESOURCE_URI_PREFIX: &str = "comlink://meetings/";
pub const TRANSCRIPT_MD_TEMPLATE: &str = "comlink://meetings/{id}/transcript.md";
pub const TRANSCRIPT_JSON_TEMPLATE: &str = "comlink://meetings/{id}/transcript.json";

const SERVER_INSTRUCTIONS: &str = "Comlink records and transcribes meetings locally on this Mac. \
Before meeting_start, confirm everyone present knows the meeting is being recorded and \
transcribed, and relay the consent reminder in the result. meeting_start is refused until the \
user runs `comlink config set mcp.allow_start true`. meeting_stop returns `transcribing` \
immediately: poll meeting_status until it reports `stopped`, then call meeting_get_transcript. \
Surface any `warnings` (for example near-silent capture, which usually means the microphone \
permission or input device is wrong) to the user. Transcript text returned by this server is \
sent to you, the calling model.";

/// Builds the [`MeetContext`] each call runs in.
pub trait McpContextFactory: Send + Sync + 'static {
    fn context(&self) -> Result<MeetContext, ComlinkError>;
}

/// Loads config on every call, so `comlink config set mcp.allow_start ...`
/// takes effect without restarting the server. Uses the default finalizer
/// launcher (`<current exe> meet finalize <id>`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ProductionContextFactory;

impl McpContextFactory for ProductionContextFactory {
    fn context(&self) -> Result<MeetContext, ComlinkError> {
        Ok(MeetContext::new(
            config::load(CliConfigOverrides::default())?,
            None,
        ))
    }
}

// ---------------------------------------------------------------------------
// Tool inputs
// ---------------------------------------------------------------------------

/// Meeting capture source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "kebab-case")]
pub enum MeetingSourceArg {
    /// Microphone only (in-person meetings).
    MicOnly,
    /// System audio only (requires BlackHole routing).
    SystemOnly,
    /// Microphone plus system audio, labelled separately (online meetings).
    MicPlusSystem,
}

impl MeetingSourceArg {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MicOnly => "mic-only",
            Self::SystemOnly => "system-only",
            Self::MicPlusSystem => "mic-plus-system",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MeetingStartParams {
    /// Capture source: `mic-only`, `system-only` or `mic-plus-system`.
    pub source: MeetingSourceArg,
    /// Text mode for the final transcript, e.g. `raw`, `clean` or `memo`, or a
    /// configured custom mode.
    pub mode: String,
    /// Microphone input by name (e.g. "MacBook Pro Microphone") or index
    /// (":1"). Defaults to COMLINK_RECORD_DEVICE, then the system default input.
    #[serde(default)]
    pub device: Option<String>,
    /// System-audio (BlackHole) input by name or index. Defaults to
    /// COMLINK_SYSTEM_AUDIO_DEVICE or a detected BlackHole device.
    #[serde(default)]
    pub system_device: Option<String>,
    /// Skip any configured local LLM rewrite (deterministic output only).
    #[serde(default)]
    pub no_llm: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MeetingIdParams {
    /// Meeting session id. Omit for the default session.
    #[serde(default)]
    pub id: Option<String>,
}

/// Transcript format.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "lowercase")]
pub enum TranscriptFormatArg {
    /// Markdown export (a string).
    Md,
    /// JSON export (`comlink.meeting.v1` object).
    Json,
}

impl TranscriptFormatArg {
    fn kind(self) -> meet::MeetingExportKind {
        match self {
            Self::Md => meet::MeetingExportKind::Markdown,
            Self::Json => meet::MeetingExportKind::Json,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MeetingTranscriptParams {
    /// Meeting session id. Omit for the newest stopped meeting (an error if a
    /// newer meeting is still transcribing or failed).
    #[serde(default)]
    pub id: Option<String>,
    /// `md` or `json`.
    pub format: TranscriptFormatArg,
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ComlinkMcp {
    factory: Arc<dyn McpContextFactory>,
    tool_router: ToolRouter<Self>,
}

impl ComlinkMcp {
    pub fn new(factory: Arc<dyn McpContextFactory>) -> Self {
        Self {
            factory,
            tool_router: Self::tool_router(),
        }
    }

    /// Run `call` with a fresh context on the blocking pool.
    async fn blocking<T, F>(&self, call: F) -> Result<T, CallFailure>
    where
        T: Send + 'static,
        F: FnOnce(MeetContext) -> Result<T, ComlinkError> + Send + 'static,
    {
        let factory = self.factory.clone();
        match tokio::task::spawn_blocking(move || call(factory.context()?)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(CallFailure::Service(error)),
            Err(join_error) if join_error.is_panic() => {
                Err(CallFailure::Panic(panic_detail(join_error.into_panic())))
            }
            Err(join_error) => Err(CallFailure::Cancelled(join_error.to_string())),
        }
    }
}

/// Why a service call did not return a value.
#[derive(Debug)]
enum CallFailure {
    Service(ComlinkError),
    Panic(String),
    Cancelled(String),
}

/// The text of a panic payload (`panic!` with a literal or a format string).
fn panic_detail(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

impl CallFailure {
    fn code(&self) -> &'static str {
        match self {
            Self::Service(error) => error.error_code(),
            Self::Panic(_) => INTERNAL_PANIC_CODE,
            Self::Cancelled(_) => INTERNAL_CANCELLED_CODE,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Service(error) => error.to_string(),
            Self::Panic(detail) => format!("comlink service call panicked: {detail}"),
            Self::Cancelled(detail) => format!("comlink service call was cancelled: {detail}"),
        }
    }

    /// Tool-level error: `isError: true` with `{error_code, message}`.
    fn into_tool_result(self) -> CallToolResult {
        let code = self.code();
        let message = self.message();
        let mut result = CallToolResult::structured_error(json!({
            "error_code": code,
            "message": message,
        }));
        result.content = vec![ContentBlock::text(format!("error ({code}): {message}"))];
        result
    }

    /// Resource reads cannot carry `isError`, so failures are JSON-RPC errors
    /// with `data: {error_code, message}`.
    fn into_resource_error(self) -> McpError {
        let code = self.code();
        let message = self.message();
        let data = Some(json!({ "error_code": code, "message": message }));
        match &self {
            Self::Service(ComlinkError::MeetingSessionNotFound(_)) => {
                McpError::resource_not_found(message, data)
            }
            Self::Service(
                ComlinkError::MeetingStillTranscribing(_)
                | ComlinkError::MeetingFinalizeFailed(_)
                | ComlinkError::MeetingFinalizeFailedDetail { .. }
                | ComlinkError::MeetingNotStopped(_)
                | ComlinkError::MeetingExportUnavailable(_)
                | ComlinkError::MeetingNoActiveSession,
            ) => McpError::invalid_request(message, data),
            _ => McpError::internal_error(message, data),
        }
    }
}

/// Successful tool result: `structuredContent` is the service JSON; the text
/// blocks are `lead` (human-readable lines) followed by the JSON itself.
fn tool_success<T: Serialize>(value: &T, lead: Vec<String>) -> CallToolResult {
    match serde_json::to_value(value) {
        Ok(json) => {
            let mut result = CallToolResult::structured(json.clone());
            let mut content: Vec<ContentBlock> = lead.into_iter().map(ContentBlock::text).collect();
            content.push(ContentBlock::text(
                serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string()),
            ));
            result.content = content;
            result
        }
        Err(error) => CallFailure::Service(ComlinkError::Json(error)).into_tool_result(),
    }
}

/// The microphone a `meeting_start` opened, as structured data for the agent.
fn input_device_json(device: &ResolvedRecordDevice) -> Value {
    json!({
        "avfoundation_input": device.avfoundation_input,
        "name": device.name,
        "selected_by": device.source.as_str(),
    })
}

/// One line telling the agent which microphone is being recorded. The
/// system-default case is the exact line `meet start` prints to stderr.
fn input_device_line(device: &ResolvedRecordDevice) -> String {
    let named = match &device.name {
        Some(name) => format!("{name} ({})", device.avfoundation_input),
        None => device.avfoundation_input.clone(),
    };
    match device.source {
        DeviceSource::SystemDefault => meet_service::DeviceNote {
            name: device.name.clone(),
            avfoundation_input: device.avfoundation_input.clone(),
        }
        .to_string(),
        DeviceSource::Fallback => format!(
            "Using fallback input device {named}: no system default input was found. If the capture is near-silent, pass `device`."
        ),
        DeviceSource::Flag => format!("Using input device {named} (from the `device` argument)"),
        DeviceSource::Env => format!("Using input device {named} (from COMLINK_RECORD_DEVICE)"),
    }
}

fn warning_lines(warnings: &[String]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| format!("warning: {warning}"))
        .collect()
}

/// Session ids are directory names in the meeting store. Anything that could
/// escape it (`/`, `..`, NUL) or is empty is rejected as not found before it
/// reaches the store.
fn checked_session_id(id: Option<String>) -> Result<Option<String>, ComlinkError> {
    match id {
        Some(id) if !is_plausible_session_id(&id) => Err(ComlinkError::MeetingSessionNotFound(id)),
        other => Ok(other),
    }
}

fn is_plausible_session_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[tool_router(router = tool_router)]
impl ComlinkMcp {
    /// Start recording a meeting locally. Requires the user to have run
    /// `comlink config set mcp.allow_start true`. The result includes a
    /// consent reminder: tell the user, and make sure everyone present agrees
    /// to being recorded and transcribed. Only one meeting records at a time.
    #[tool(
        name = "meeting_start",
        annotations(
            title = "Start meeting recording",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn meeting_start(
        &self,
        Parameters(params): Parameters<MeetingStartParams>,
    ) -> Result<CallToolResult, McpError> {
        let request_source = params.source;
        let outcome = self
            .blocking(move |base| {
                if !base.resolved.config.mcp.allow_start {
                    return Err(ComlinkError::McpStartDisabled);
                }
                let prepared = meet_service::prepare_start(
                    &base.resolved,
                    base.runtime.clone(),
                    meet_service::StartOptions {
                        mode: params.mode,
                        source: params.source.as_str().to_string(),
                        device: params.device,
                        system_device: params.system_device,
                        chunk_seconds: meet_service::DEFAULT_CHUNK_SECONDS,
                        no_llm: params.no_llm.unwrap_or(false),
                    },
                )?;
                // System-only capture opens no microphone.
                let mic = (request_source != MeetingSourceArg::SystemOnly)
                    .then(|| prepared.input_device.clone());
                let (ctx, request, _note) = prepared.into_context(base.resolved.clone());
                let ctx = ctx.with_launcher(base.launcher.clone());
                Ok((meet_service::start(&ctx, request)?, mic))
            })
            .await;
        Ok(match outcome {
            Ok((status, mic)) => {
                let mut lead = vec![status.consent_reminder.to_string()];
                if let Some(mic) = &mic {
                    lead.push(input_device_line(mic));
                }
                lead.push(format!(
                    "Recording meeting {}. Call meeting_stop to stop, then poll meeting_status until `stopped`.",
                    status.session_id
                ));
                match serde_json::to_value(&status) {
                    Ok(mut json) => {
                        json["input_device"] =
                            mic.as_ref().map(input_device_json).unwrap_or(Value::Null);
                        tool_success(&json, lead)
                    }
                    Err(error) => {
                        CallFailure::Service(ComlinkError::Json(error)).into_tool_result()
                    }
                }
            }
            Err(failure) => failure.into_tool_result(),
        })
    }

    /// Report a meeting's state: `recording`, `transcribing`, `stopped`,
    /// `failed`, or `none`. With no id: the active recording, else the newest
    /// transcribing meeting, else a newer failed one. Includes recorder and
    /// finalizer health, the latest audio level, `stale`/`stale_reason`, the
    /// finalize `error` and `finalize_log`, and `warnings` (e.g. near-silent
    /// capture). Poll this after meeting_stop until status is `stopped`.
    #[tool(
        name = "meeting_status",
        annotations(
            title = "Meeting status",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn meeting_status(
        &self,
        Parameters(params): Parameters<MeetingIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let outcome = self
            .blocking(move |ctx| meet_service::status(&ctx, checked_session_id(params.id)?))
            .await;
        Ok(match outcome {
            Ok(report) => {
                let mut lead = vec![match &report.session_id {
                    Some(id) => format!("Meeting {id}: {}", report.status),
                    None => "No meeting session (status none).".to_string(),
                }];
                lead.extend(warning_lines(&report.warnings));
                tool_success(&report, lead)
            }
            Err(failure) => failure.into_tool_result(),
        })
    }

    /// Stop the recording (the active one, or the given id) and return
    /// immediately with status `transcribing`; transcription finishes in a
    /// detached background process. Poll meeting_status until it reports
    /// `stopped` (or `failed`), then call meeting_get_transcript. When
    /// retention.audio is off, the audio chunks are deleted after
    /// transcription.
    #[tool(
        name = "meeting_stop",
        annotations(
            title = "Stop meeting recording",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn meeting_stop(
        &self,
        Parameters(params): Parameters<MeetingIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let outcome = self
            .blocking(move |ctx| {
                let session = meet_service::prepare_stop(&ctx, checked_session_id(params.id)?)?;
                meet_service::stop_detached_prepared(
                    &ctx,
                    &session.session_id,
                    meet_service::DEFAULT_STOP_TIMEOUT,
                )
            })
            .await;
        Ok(match outcome {
            Ok(status) => {
                let lead = vec![format!(
                    "Stopped recording meeting {}; transcribing in the background. Poll meeting_status until status is `stopped`, then call meeting_get_transcript.",
                    status.session_id
                )];
                tool_success(&status, lead)
            }
            Err(failure) => failure.into_tool_result(),
        })
    }

    /// Return a stopped meeting's transcript as Markdown (`md`) or as the
    /// `comlink.meeting.v1` JSON export (`json`), with its warnings and audio
    /// level. Errors while the meeting is still recording or transcribing, if
    /// finalize failed (with the error and a retry command), or if there is no
    /// meeting. The transcript text is sent to you, the calling model.
    #[tool(
        name = "meeting_get_transcript",
        annotations(
            title = "Get meeting transcript",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn meeting_get_transcript(
        &self,
        Parameters(params): Parameters<MeetingTranscriptParams>,
    ) -> Result<CallToolResult, McpError> {
        let kind = params.format.kind();
        let outcome = self
            .blocking(move |ctx| {
                meet_service::transcript(&ctx, checked_session_id(params.id)?, kind)
            })
            .await;
        Ok(match outcome {
            Ok(transcript) => {
                let mut lead = warning_lines(&transcript.warnings);
                if !transcript.transcript_retained {
                    lead.push(
                        "note: retention.transcripts is off for this meeting, so the transcript text was not kept.".to_string(),
                    );
                }
                let mut result = tool_success(&transcript, lead);
                // For Markdown, the last text block is the transcript itself
                // rather than the JSON mirror of it.
                if let Value::String(markdown) = &transcript.content {
                    if let Some(last) = result.content.last_mut() {
                        *last = ContentBlock::text(markdown.clone());
                    }
                }
                result
            }
            Err(failure) => failure.into_tool_result(),
        })
    }

    /// List meeting sessions, newest first, with id, status, start/stop time,
    /// duration and source mode. Unreadable session directories are listed
    /// under `skipped`.
    #[tool(
        name = "meeting_list",
        annotations(
            title = "List meetings",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn meeting_list(&self) -> Result<CallToolResult, McpError> {
        let outcome = self.blocking(|ctx| meet_service::list(&ctx)).await;
        Ok(match outcome {
            Ok(list) => {
                let lead = vec![format!("{} meeting session(s).", list.sessions.len())];
                tool_success(&list, lead)
            }
            Err(failure) => failure.into_tool_result(),
        })
    }
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// A parsed `comlink://meetings/{id}/transcript.{md,json}` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptUri {
    pub id: String,
    pub kind: TranscriptUriKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptUriKind {
    Markdown,
    Json,
}

impl TranscriptUriKind {
    fn mime_type(self) -> &'static str {
        match self {
            Self::Markdown => "text/markdown",
            Self::Json => "application/json",
        }
    }

    fn export_kind(self) -> meet::MeetingExportKind {
        match self {
            Self::Markdown => meet::MeetingExportKind::Markdown,
            Self::Json => meet::MeetingExportKind::Json,
        }
    }
}

pub fn transcript_uri(id: &str, kind: TranscriptUriKind) -> String {
    let suffix = match kind {
        TranscriptUriKind::Markdown => "transcript.md",
        TranscriptUriKind::Json => "transcript.json",
    };
    format!("{RESOURCE_URI_PREFIX}{}/{suffix}", percent_encode(id))
}

/// Strict parse: the prefix, exactly one percent-decoded id segment that is a
/// plausible session id, and a `transcript.md` / `transcript.json` suffix.
pub fn parse_transcript_uri(uri: &str) -> Result<TranscriptUri, String> {
    let rest = uri
        .strip_prefix(RESOURCE_URI_PREFIX)
        .ok_or_else(|| format!("unsupported resource URI {uri}; expected {TRANSCRIPT_MD_TEMPLATE} or {TRANSCRIPT_JSON_TEMPLATE}"))?;
    let (raw_id, suffix) = rest
        .split_once('/')
        .ok_or_else(|| format!("resource URI {uri} has no transcript suffix"))?;
    let kind = match suffix {
        "transcript.md" => TranscriptUriKind::Markdown,
        "transcript.json" => TranscriptUriKind::Json,
        other => {
            return Err(format!(
                "unsupported transcript resource {other:?} in {uri}; use transcript.md or transcript.json"
            ))
        }
    };
    let id = percent_decode(raw_id).ok_or_else(|| format!("invalid percent-encoding in {uri}"))?;
    if !is_plausible_session_id(&id) {
        return Err(format!("invalid meeting session id {id:?} in {uri}"));
    }
    Ok(TranscriptUri { id, kind })
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = input.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn percent_encode(input: &str) -> String {
    input
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn resource_templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new(TRANSCRIPT_MD_TEMPLATE, "meeting-transcript-md")
            .with_title("Meeting transcript (Markdown)")
            .with_description("Markdown transcript of a stopped Comlink meeting. Sent to the calling model.")
            .with_mime_type(TranscriptUriKind::Markdown.mime_type()),
        ResourceTemplate::new(TRANSCRIPT_JSON_TEMPLATE, "meeting-transcript-json")
            .with_title("Meeting transcript (JSON)")
            .with_description("comlink.meeting.v1 JSON export of a stopped Comlink meeting. Sent to the calling model.")
            .with_mime_type(TranscriptUriKind::Json.mime_type()),
    ]
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ComlinkMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(LATEST_PROTOCOL_VERSION)
        .with_server_info(Implementation::new("comlink", env!("CARGO_PKG_VERSION")))
        .with_instructions(SERVER_INSTRUCTIONS)
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(SUPPORTED_PROTOCOL_VERSIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let list = self
            .blocking(|ctx| meet_service::list(&ctx))
            .await
            .map_err(CallFailure::into_resource_error)?;
        let resources = list
            .sessions
            .iter()
            .filter(|session| session.status == meet::MeetingStatus::Stopped.as_str())
            .flat_map(|session| {
                [TranscriptUriKind::Markdown, TranscriptUriKind::Json].map(|kind| {
                    let label = match kind {
                        TranscriptUriKind::Markdown => "Markdown",
                        TranscriptUriKind::Json => "JSON",
                    };
                    Resource::new(
                        transcript_uri(&session.session_id, kind),
                        format!("{} transcript ({label})", session.session_id),
                    )
                    .with_mime_type(kind.mime_type())
                })
            })
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(
            resource_templates(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri;
        let parsed = parse_transcript_uri(&uri).map_err(|message| {
            McpError::invalid_params(
                message.clone(),
                Some(json!({ "error_code": INVALID_RESOURCE_URI_CODE, "message": message })),
            )
        })?;
        let kind = parsed.kind;
        let transcript = self
            .blocking(move |ctx| {
                meet_service::transcript(&ctx, Some(parsed.id), kind.export_kind())
            })
            .await
            .map_err(CallFailure::into_resource_error)?;
        let text = match &transcript.content {
            Value::String(markdown) => markdown.clone(),
            other => serde_json::to_string_pretty(other).map_err(|error| {
                CallFailure::Service(ComlinkError::Json(error)).into_resource_error()
            })?,
        };
        let contents = ResourceContents::text(text, uri).with_mime_type(kind.mime_type());
        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

/// `comlink mcp`: serve MCP over stdin/stdout until the client disconnects.
pub fn serve_stdio() -> Result<(), ComlinkError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let server = ComlinkMcp::new(Arc::new(ProductionContextFactory))
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| std::io::Error::other(format!("MCP initialize failed: {error}")))?;
        server
            .waiting()
            .await
            .map_err(|error| std::io::Error::other(format!("MCP transport failed: {error}")))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_uris_round_trip_and_reject_anything_else() {
        let uri = transcript_uri("meeting-1", TranscriptUriKind::Markdown);
        assert_eq!(uri, "comlink://meetings/meeting-1/transcript.md");
        assert_eq!(
            parse_transcript_uri(&uri).unwrap(),
            TranscriptUri {
                id: "meeting-1".to_string(),
                kind: TranscriptUriKind::Markdown
            }
        );
        assert_eq!(
            parse_transcript_uri("comlink://meetings/m%2D1/transcript.json")
                .unwrap()
                .id,
            "m-1"
        );
        for bad in [
            "file:///etc/passwd",
            "comlink://meetings/",
            "comlink://meetings/m1",
            "comlink://meetings/m1/transcript.txt",
            "comlink://meetings/m1/transcript.md/extra",
            "comlink://meetings//transcript.md",
            "comlink://meetings/../transcript.md",
            "comlink://meetings/%2E%2E/transcript.md",
            "comlink://meetings/a%2Fb/transcript.md",
            "comlink://meetings/a%2/transcript.md",
            "comlink://meetings/a%00/transcript.md",
            "comlink://other/m1/transcript.md",
        ] {
            assert!(
                parse_transcript_uri(bad).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn session_ids_that_could_escape_the_store_are_not_found() {
        for bad in ["", ".", "..", "../x", "a/b", "a\0b", "a..b"] {
            let error = checked_session_id(Some(bad.to_string())).unwrap_err();
            assert_eq!(error.error_code(), "meeting_session_not_found");
        }
        assert_eq!(checked_session_id(None).unwrap(), None);
        assert_eq!(
            checked_session_id(Some("meeting-20260924-abc_1".to_string())).unwrap(),
            Some("meeting-20260924-abc_1".to_string())
        );
    }

    #[test]
    fn every_source_arg_is_a_service_source_mode() {
        for (arg, mode) in [
            (MeetingSourceArg::MicOnly, meet::MeetSourceMode::MicOnly),
            (
                MeetingSourceArg::SystemOnly,
                meet::MeetSourceMode::SystemOnly,
            ),
            (
                MeetingSourceArg::MicPlusSystem,
                meet::MeetSourceMode::MicPlusSystem,
            ),
        ] {
            assert_eq!(meet::MeetSourceMode::parse(arg.as_str()), Some(mode));
            // The wire name the schema advertises is the one the service parses.
            let wire: MeetingSourceArg = serde_json::from_value(json!(arg.as_str())).unwrap();
            assert_eq!(wire, arg);
        }
    }

    #[test]
    fn input_device_line_names_the_microphone_for_every_selection() {
        let device = |name: Option<&str>, source| ResolvedRecordDevice {
            avfoundation_input: ":1".to_string(),
            name: name.map(str::to_string),
            source,
        };
        // System default: the historical `meet start` stderr line.
        assert_eq!(
            input_device_line(&device(
                Some("MacBook Pro Microphone"),
                DeviceSource::SystemDefault
            )),
            "Using system default input device: MacBook Pro Microphone (:1)"
        );
        assert_eq!(
            input_device_line(&device(None, DeviceSource::SystemDefault)),
            "Using system default input device :1"
        );
        let fallback = input_device_line(&device(None, DeviceSource::Fallback));
        assert!(
            fallback.starts_with("Using fallback input device :1"),
            "{fallback}"
        );
        assert!(fallback.contains("pass `device`"), "{fallback}");
        assert_eq!(
            input_device_line(&device(Some("USB Mic"), DeviceSource::Flag)),
            "Using input device USB Mic (:1) (from the `device` argument)"
        );
        assert_eq!(
            input_device_line(&device(None, DeviceSource::Env)),
            "Using input device :1 (from COMLINK_RECORD_DEVICE)"
        );
        let json = input_device_json(&device(Some("USB Mic"), DeviceSource::Env));
        assert_eq!(
            json,
            json!({"avfoundation_input": ":1", "name": "USB Mic", "selected_by": "COMLINK_RECORD_DEVICE"})
        );
    }

    #[test]
    fn panic_payload_text_is_kept() {
        assert_eq!(panic_detail(Box::new("boom")), "boom");
        assert_eq!(panic_detail(Box::new(String::from("kaboom 7"))), "kaboom 7");
        assert_eq!(panic_detail(Box::new(7_u8)), "non-string panic payload");
    }

    #[test]
    fn supported_versions_stop_at_the_latest_initialize_revision() {
        assert!(SUPPORTED_PROTOCOL_VERSIONS.contains(&LATEST_PROTOCOL_VERSION));
        assert_eq!(
            SUPPORTED_PROTOCOL_VERSIONS.last(),
            Some(&LATEST_PROTOCOL_VERSION)
        );
        assert!(!SUPPORTED_PROTOCOL_VERSIONS.contains(&ProtocolVersion::V_2024_11_05));
        assert!(!SUPPORTED_PROTOCOL_VERSIONS.contains(&ProtocolVersion::V_2026_07_28));
    }
}
