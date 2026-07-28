use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    asr::Transcript,
    config::{ConfigPaths, RetentionConfig},
    error::ComlinkError,
    output::{self, TextProcessingResult},
    record::SegmentedCaptureIdentity,
};

pub const MEETING_SCHEMA_VERSION: &str = "comlink.meeting.v1";
const ACTIVE_SESSION_FILE: &str = "active-session";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MeetSourceMode {
    MicOnly,
    SystemOnly,
    MicPlusSystem,
}

impl MeetSourceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MicOnly => "mic-only",
            Self::SystemOnly => "system-only",
            Self::MicPlusSystem => "mic-plus-system",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mic-only" => Some(Self::MicOnly),
            "system-only" => Some(Self::SystemOnly),
            "mic-plus-system" => Some(Self::MicPlusSystem),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingSourceLabel {
    UserMic,
    SystemAudio,
    Mixed,
}

impl MeetingSourceLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserMic => "user_mic",
            Self::SystemAudio => "system_audio",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingSourceStream {
    pub label: MeetingSourceLabel,
    pub device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorder_stderr_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorder_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorder_identity: Option<SegmentedCaptureIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingSourceMetadata {
    pub mode: MeetSourceMode,
    pub streams: Vec<MeetingSourceStream>,
}

impl MeetingSourceMetadata {
    pub fn new(mode: MeetSourceMode, streams: Vec<MeetingSourceStream>) -> Self {
        Self { mode, streams }
    }

    pub fn redacted(&self) -> Self {
        Self {
            mode: self.mode,
            streams: self
                .streams
                .iter()
                .map(|stream| MeetingSourceStream {
                    label: stream.label,
                    device: "<redacted>".to_string(),
                    chunks_dir: None,
                    recorder_stderr_path: None,
                    recorder_pid: None,
                    recorder_identity: None,
                })
                .collect(),
        }
    }

    pub fn export_metadata(&self, retain_metadata: bool) -> Self {
        Self {
            mode: self.mode,
            streams: self
                .streams
                .iter()
                .map(|stream| MeetingSourceStream {
                    label: stream.label,
                    device: if retain_metadata {
                        stream.device.clone()
                    } else {
                        "<redacted>".to_string()
                    },
                    chunks_dir: None,
                    recorder_stderr_path: None,
                    recorder_pid: None,
                    recorder_identity: None,
                })
                .collect(),
        }
    }

    pub fn summary(&self) -> String {
        self.streams
            .iter()
            .map(|stream| format!("{} device {}", stream.label.as_str(), stream.device))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn with_session_paths(
        mut self,
        chunks_dir: &Path,
        recorder_stderr_path: &Path,
        use_legacy_single_stream_paths: bool,
    ) -> Self {
        let multi_stream = self.streams.len() > 1;
        for stream in &mut self.streams {
            if use_legacy_single_stream_paths && !multi_stream {
                stream.chunks_dir = Some(chunks_dir.display().to_string());
                stream.recorder_stderr_path = Some(recorder_stderr_path.display().to_string());
                continue;
            }

            let label = stream.label.as_str();
            stream.chunks_dir = Some(chunks_dir.join(label).display().to_string());
            stream.recorder_stderr_path = Some(
                recorder_stderr_path
                    .with_file_name(format!("capture-{label}.stderr"))
                    .display()
                    .to_string(),
            );
        }
        self
    }
}

impl Default for MeetingSourceMetadata {
    fn default() -> Self {
        Self {
            mode: MeetSourceMode::MicOnly,
            streams: vec![MeetingSourceStream {
                label: MeetingSourceLabel::UserMic,
                device: ":0".to_string(),
                chunks_dir: None,
                recorder_stderr_path: None,
                recorder_pid: None,
                recorder_identity: None,
            }],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingStatus {
    Recording,
    Stopped,
}

impl MeetingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingRetentionPolicy {
    pub metadata: bool,
    pub transcripts: bool,
    pub audio: bool,
}

impl From<&RetentionConfig> for MeetingRetentionPolicy {
    fn from(retention: &RetentionConfig) -> Self {
        Self {
            metadata: retention.metadata,
            transcripts: retention.transcripts,
            audio: retention.audio,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InactivityAutoStop {
    pub enabled: bool,
    pub reason: String,
}

impl InactivityAutoStop {
    pub fn disabled_known_limitation() -> Self {
        Self {
            enabled: false,
            reason:
                "not enabled in Phase 7; the current capture adapter has no reliable realtime silence signal"
                    .to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingSessionState {
    pub schema_version: String,
    pub session_id: String,
    pub status: MeetingStatus,
    pub started_at_ms: i64,
    pub stopped_at_ms: Option<i64>,
    pub duration_ms: Option<u64>,
    pub mode: String,
    pub no_llm: bool,
    pub recorder_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorder_identity: Option<SegmentedCaptureIdentity>,
    pub device: String,
    #[serde(default)]
    pub source: MeetingSourceMetadata,
    pub chunk_duration_ms: u64,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub engine: String,
    pub model: String,
    pub model_path: String,
    pub retention: MeetingRetentionPolicy,
    pub inactivity_auto_stop: InactivityAutoStop,
    pub session_dir: String,
    pub chunks_dir: String,
    pub recorder_stderr_path: String,
    pub segments_jsonl_path: String,
    pub json_export_path: String,
    pub markdown_export_path: String,
    pub segment_count: usize,
}

impl MeetingSessionState {
    pub fn mark_stopped(&mut self, stopped_at_ms: i64, duration_ms: u64, segment_count: usize) {
        self.status = MeetingStatus::Stopped;
        self.stopped_at_ms = Some(stopped_at_ms);
        self.duration_ms = Some(duration_ms);
        self.recorder_pid = None;
        self.recorder_identity = None;
        self.segment_count = segment_count;
    }
}

#[derive(Debug, Clone)]
pub struct NewMeetingSession {
    pub session_id: String,
    pub mode: String,
    pub no_llm: bool,
    pub device: String,
    pub source: MeetingSourceMetadata,
    pub chunk_duration_ms: u64,
    pub model: String,
    pub model_path: String,
    pub retention: MeetingRetentionPolicy,
    pub session_dir: PathBuf,
    pub chunks_dir: PathBuf,
    pub recorder_stderr_path: PathBuf,
    pub segments_jsonl_path: PathBuf,
    pub json_export_path: PathBuf,
    pub markdown_export_path: PathBuf,
}

pub fn new_recording_session(options: NewMeetingSession) -> MeetingSessionState {
    MeetingSessionState {
        schema_version: MEETING_SCHEMA_VERSION.to_string(),
        session_id: options.session_id,
        status: MeetingStatus::Recording,
        started_at_ms: now_ms(),
        stopped_at_ms: None,
        duration_ms: None,
        mode: options.mode,
        no_llm: options.no_llm,
        recorder_pid: None,
        recorder_identity: None,
        device: options.device,
        source: options.source,
        chunk_duration_ms: options.chunk_duration_ms,
        sample_rate_hz: 16_000,
        channels: 1,
        engine: "whisper.cpp".to_string(),
        model: options.model,
        model_path: options.model_path,
        retention: options.retention,
        inactivity_auto_stop: InactivityAutoStop::disabled_known_limitation(),
        session_dir: options.session_dir.display().to_string(),
        chunks_dir: options.chunks_dir.display().to_string(),
        recorder_stderr_path: options.recorder_stderr_path.display().to_string(),
        segments_jsonl_path: options.segments_jsonl_path.display().to_string(),
        json_export_path: options.json_export_path.display().to_string(),
        markdown_export_path: options.markdown_export_path.display().to_string(),
        segment_count: 0,
    }
}

#[derive(Debug, Clone)]
pub struct MeetingChunk {
    pub index: usize,
    pub path: PathBuf,
    pub source_label: MeetingSourceLabel,
    pub source_device: String,
    pub start_ms: u64,
    pub duration_ms: u64,
}

impl MeetingChunk {
    pub fn end_ms(&self) -> u64 {
        self.start_ms.saturating_add(self.duration_ms)
    }
}

#[derive(Debug, Clone)]
pub struct ChunkTranscript {
    pub chunk_index: usize,
    pub chunk_path: PathBuf,
    pub source_label: MeetingSourceLabel,
    pub source_device: String,
    pub chunk_start_ms: u64,
    pub chunk_duration_ms: u64,
    pub transcript: Transcript,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingSegment {
    pub segment_index: usize,
    pub chunk_index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub source_label: MeetingSourceLabel,
    pub source_device: String,
    pub text: String,
    pub chunk_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingSegmenting {
    pub strategy: String,
    pub vad_available: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentBuildResult {
    pub segments: Vec<MeetingSegment>,
    pub segmenting: MeetingSegmenting,
}

pub fn build_segments_from_chunk_transcripts(chunks: &[ChunkTranscript]) -> SegmentBuildResult {
    let mut segments = Vec::new();
    let mut vad_available = false;

    for chunk in chunks {
        let source_segments = if chunk.transcript.segments.is_empty() {
            vec![crate::asr::Segment {
                start_ms: 0,
                end_ms: chunk.chunk_duration_ms,
                text: chunk.transcript.text.clone(),
            }]
        } else {
            chunk.transcript.segments.clone()
        };

        if source_segments.len() > 1
            || source_segments.iter().any(|segment| {
                segment.start_ms > 0 || segment.end_ms < chunk.chunk_duration_ms.saturating_sub(1)
            })
        {
            vad_available = true;
        }

        for source_segment in source_segments {
            let text = source_segment.text.trim();
            if text.is_empty() {
                continue;
            }

            let start_ms = chunk
                .chunk_start_ms
                .saturating_add(source_segment.start_ms.min(chunk.chunk_duration_ms));
            let mut end_ms = chunk
                .chunk_start_ms
                .saturating_add(source_segment.end_ms.min(chunk.chunk_duration_ms));
            if end_ms <= start_ms {
                end_ms = start_ms.saturating_add(1);
            }

            segments.push(MeetingSegment {
                segment_index: segments.len(),
                chunk_index: chunk.chunk_index,
                start_ms,
                end_ms,
                source_label: chunk.source_label,
                source_device: chunk.source_device.clone(),
                text: text.to_string(),
                chunk_path: chunk.chunk_path.display().to_string(),
            });
        }
    }

    segments.sort_by(|left, right| {
        left.start_ms
            .cmp(&right.start_ms)
            .then_with(|| left.end_ms.cmp(&right.end_ms))
            .then_with(|| left.source_label.as_str().cmp(right.source_label.as_str()))
            .then_with(|| left.chunk_path.cmp(&right.chunk_path))
    });
    for (index, segment) in segments.iter_mut().enumerate() {
        segment.segment_index = index;
    }

    let segmenting = if vad_available {
        MeetingSegmenting {
            strategy: "asr-segments-offset-by-chunk".to_string(),
            vad_available: true,
            detail: "ASR supplied sub-chunk segment timing; Comlink offset it into meeting time"
                .to_string(),
        }
    } else {
        MeetingSegmenting {
            strategy: "chunk-boundaries".to_string(),
            vad_available: false,
            detail: "ASR did not expose VAD segment timing; Comlink used capture chunk boundaries"
                .to_string(),
        }
    };

    SegmentBuildResult {
        segments,
        segmenting,
    }
}

pub fn raw_text_from_segments(segments: &[MeetingSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingArtifacts {
    pub session_dir: String,
    pub segments_jsonl: String,
    pub json_export: String,
    pub markdown_export: String,
    pub chunks_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MeetingAudioLevel {
    pub mean_dbfs: f64,
    pub peak_dbfs: f64,
    pub near_silent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingExportSession {
    pub session_id: String,
    pub status: String,
    pub started_at_ms: i64,
    pub stopped_at_ms: Option<i64>,
    pub duration_ms: u64,
    pub mode: String,
    pub source: String,
    pub source_mode: MeetSourceMode,
    pub source_streams: Vec<MeetingSourceStream>,
    pub chunk_duration_ms: u64,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub engine: String,
    pub model: String,
    pub segment_count: usize,
    pub inactivity_auto_stop: InactivityAutoStop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingSegmentExport {
    pub segment_index: usize,
    pub chunk_index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub source_label: MeetingSourceLabel,
    pub source_device: Option<String>,
    pub text: Option<String>,
    pub chunk_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingExport {
    pub schema_version: String,
    pub session: MeetingExportSession,
    pub source: MeetingSourceMetadata,
    pub retention: MeetingRetentionPolicy,
    pub segmenting: MeetingSegmenting,
    pub artifacts: MeetingArtifacts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_level: Option<MeetingAudioLevel>,
    pub raw_text: Option<String>,
    pub final_text: Option<String>,
    pub processing_steps: Vec<String>,
    pub warnings: Vec<String>,
    pub segments: Vec<MeetingSegmentExport>,
}

pub fn build_export(
    session: &MeetingSessionState,
    segments: &[MeetingSegment],
    raw_text: &str,
    processed: &TextProcessingResult,
    segmenting: MeetingSegmenting,
    audio_level: Option<MeetingAudioLevel>,
) -> MeetingExport {
    let retention = session.retention.clone();
    let artifacts = MeetingArtifacts {
        session_dir: session.session_dir.clone(),
        segments_jsonl: session.segments_jsonl_path.clone(),
        json_export: session.json_export_path.clone(),
        markdown_export: session.markdown_export_path.clone(),
        chunks_dir: retention.audio.then_some(session.chunks_dir.clone()),
    };
    let source_metadata = session.source.export_metadata(retention.metadata);
    let source = if retention.metadata {
        source_metadata.summary()
    } else {
        "<redacted>".to_string()
    };
    let model = if retention.metadata {
        session.model.clone()
    } else {
        "<redacted>".to_string()
    };

    let mut warnings = processed.warnings.clone();
    if let Some(level) = &audio_level {
        if level.near_silent {
            warnings.push(crate::audio::near_silent_warning_message(level.mean_dbfs));
        }
    }
    warnings.extend(transcript_quality_warnings(session, segments));
    let (final_text, final_collapsed) = collapse_repetition_loops(&processed.final_text);
    let mut exported_segments = Vec::with_capacity(segments.len());
    let mut segments_collapsed = false;
    for segment in segments {
        let (text, collapsed) = collapse_repetition_loops(&segment.text);
        segments_collapsed |= collapsed;
        exported_segments.push((segment, text));
    }
    let collapsed_repetition = final_collapsed || segments_collapsed;
    if collapsed_repetition {
        warnings.push(
            "repeated phrase loops were collapsed in final transcript and segment text; raw_text preserves the original ASR output"
                .to_string(),
        );
    }
    let mut processing_steps = processed
        .processing_steps
        .iter()
        .map(|step| step.name.clone())
        .collect::<Vec<_>>();
    if collapsed_repetition {
        processing_steps.push("meeting-repetition-loop-collapse".to_string());
    }

    MeetingExport {
        schema_version: MEETING_SCHEMA_VERSION.to_string(),
        session: MeetingExportSession {
            session_id: session.session_id.clone(),
            status: session.status.as_str().to_string(),
            started_at_ms: session.started_at_ms,
            stopped_at_ms: session.stopped_at_ms,
            duration_ms: session.duration_ms.unwrap_or_default(),
            mode: processed.mode.clone(),
            source,
            source_mode: session.source.mode,
            source_streams: source_metadata.streams.clone(),
            chunk_duration_ms: session.chunk_duration_ms,
            sample_rate_hz: session.sample_rate_hz,
            channels: session.channels,
            engine: session.engine.clone(),
            model,
            segment_count: segments.len(),
            inactivity_auto_stop: session.inactivity_auto_stop.clone(),
        },
        source: source_metadata,
        retention: retention.clone(),
        segmenting,
        artifacts,
        audio_level,
        raw_text: retention.transcripts.then(|| raw_text.to_string()),
        final_text: retention.transcripts.then_some(final_text),
        processing_steps,
        warnings,
        segments: exported_segments
            .into_iter()
            .map(|(segment, text)| MeetingSegmentExport {
                segment_index: segment.segment_index,
                chunk_index: segment.chunk_index,
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                source_label: segment.source_label,
                source_device: retention.metadata.then(|| segment.source_device.clone()),
                text: retention.transcripts.then_some(text),
                chunk_path: retention.audio.then(|| segment.chunk_path.clone()),
            })
            .collect(),
    }
}

fn transcript_quality_warnings(
    session: &MeetingSessionState,
    segments: &[MeetingSegment],
) -> Vec<String> {
    let mut warnings = Vec::new();
    let model_name = Path::new(&session.model_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&session.model_path)
        .to_ascii_lowercase();
    if model_name.contains("tiny") {
        warnings.push(
            "meeting was transcribed with a tiny Whisper model; noisy rooms usually need base.en, small.en, or larger for usable quality"
                .to_string(),
        );
    }

    let text = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if non_speech_marker_count(&text) > 0 {
        warnings.push(
            "transcript contains non-speech markers such as [BLANK_AUDIO], which usually indicates silence or background-noise hallucination"
                .to_string(),
        );
    }
    if has_repetition_loop(&text) {
        warnings.push(
            "transcript contains a repeated phrase loop; review the recording environment or rerun with a larger Whisper model"
                .to_string(),
        );
    }

    warnings
}

fn non_speech_marker_count(text: &str) -> usize {
    let text = text.to_ascii_lowercase();
    ["[blank_audio]", "[music]", "[applause]", "[laughter]"]
        .iter()
        .map(|marker| text.matches(marker).count())
        .sum()
}

/// Number of consecutive identical tokens that flags a runaway single-token
/// loop (the `you you you …` whisper-on-silence signature).
const UNIGRAM_RUN_THRESHOLD: usize = 10;
/// Minimum token count before the "one token dominates the whole transcript"
/// heuristic is allowed to fire, so short emphatic speech ("no, no, no") never
/// trips it.
const UNIGRAM_DOMINANCE_MIN_TOKENS: usize = 20;
/// Fraction of all tokens a single token must reach to count as a dominant
/// unigram loop.
const UNIGRAM_DOMINANCE_RATIO: f64 = 0.6;

fn has_repetition_loop(text: &str) -> bool {
    has_phrase_repetition_loop(text) || has_dominant_unigram_loop(text)
}

/// Detect a repeated multi-word phrase run (e.g. a whole sentence emitted five
/// times in a row).
fn has_phrase_repetition_loop(text: &str) -> bool {
    let mut previous = String::new();
    let mut run_len = 0usize;

    for unit in text
        .split(['.', '!', '?', '\n'])
        .map(normalize_repetition_unit)
        .filter(|unit| unit.split_whitespace().count() >= 2)
    {
        if unit == previous {
            run_len += 1;
        } else {
            previous = unit;
            run_len = 1;
        }

        if run_len >= 5 {
            return true;
        }
    }

    false
}

/// Detect the single-token whisper-on-silence signature that the phrase-run
/// check misses: one short token (`you`, `thank`) repeated far more than any
/// real utterance would. Trips on either a long consecutive run of the same
/// token, or a single token dominating a sufficiently long transcript.
fn has_dominant_unigram_loop(text: &str) -> bool {
    let tokens: Vec<String> = text
        .split_whitespace()
        .map(normalize_repetition_unit)
        .filter(|token| !token.is_empty())
        .collect();

    let mut run_len = 0usize;
    let mut previous: Option<&str> = None;
    for token in &tokens {
        if previous == Some(token.as_str()) {
            run_len += 1;
        } else {
            previous = Some(token.as_str());
            run_len = 1;
        }
        if run_len >= UNIGRAM_RUN_THRESHOLD {
            return true;
        }
    }

    if tokens.len() >= UNIGRAM_DOMINANCE_MIN_TOKENS {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for token in &tokens {
            *counts.entry(token.as_str()).or_default() += 1;
        }
        if let Some(max) = counts.values().copied().max() {
            if max as f64 >= tokens.len() as f64 * UNIGRAM_DOMINANCE_RATIO {
                return true;
            }
        }
    }

    false
}

fn normalize_repetition_unit(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone)]
struct RepeatUnit {
    text: String,
    normalized: String,
}

fn collapse_repetition_loops(text: &str) -> (String, bool) {
    let units = repeat_units(text);
    if units.is_empty() {
        return (text.trim().to_string(), false);
    }

    let mut collapsed = false;
    let mut output = Vec::with_capacity(units.len());
    let mut index = 0usize;
    while index < units.len() {
        let unit = &units[index];
        if unit.normalized.len() < 7 {
            output.push(unit.text.clone());
            index += 1;
            continue;
        }

        let mut end = index + 1;
        while end < units.len() && units[end].normalized == unit.normalized {
            end += 1;
        }

        let run_len = end - index;
        if run_len >= 5 {
            collapsed = true;
            output.push(units[index].text.clone());
            output.push(units[index + 1].text.clone());
            output.push("[repeated phrase loop collapsed]".to_string());
        } else {
            output.extend(units[index..end].iter().map(|unit| unit.text.clone()));
        }
        index = end;
    }

    (output.join(" ").trim().to_string(), collapsed)
}

fn repeat_units(text: &str) -> Vec<RepeatUnit> {
    let mut units = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | ',' | '\n') {
            push_repeat_unit(&mut units, &mut current);
        }
    }
    push_repeat_unit(&mut units, &mut current);

    units
}

fn push_repeat_unit(units: &mut Vec<RepeatUnit>, current: &mut String) {
    let text = current.trim();
    if !text.is_empty() {
        units.push(RepeatUnit {
            text: text.to_string(),
            normalized: normalize_repetition_unit(text),
        });
    }
    current.clear();
}

pub fn render_markdown(export: &MeetingExport) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Comlink Meeting Transcript\n\n");
    markdown.push_str("## Session Metadata\n\n");
    push_field(&mut markdown, "Schema", &export.schema_version);
    push_field(&mut markdown, "Session", &export.session.session_id);
    push_field(&mut markdown, "Status", &export.session.status);
    push_field(
        &mut markdown,
        "Started",
        &export.session.started_at_ms.to_string(),
    );
    push_field(
        &mut markdown,
        "Stopped",
        &export
            .session
            .stopped_at_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<not stopped>".to_string()),
    );
    push_field(
        &mut markdown,
        "Duration",
        &format!("{} ms", export.session.duration_ms),
    );
    push_field(&mut markdown, "Mode", &export.session.mode);
    push_field(&mut markdown, "Source", &export.session.source);
    push_field(
        &mut markdown,
        "Source mode",
        export.session.source_mode.as_str(),
    );
    for stream in &export.source.streams {
        push_field(
            &mut markdown,
            "Source stream",
            &format!("{}: {}", stream.label.as_str(), stream.device),
        );
    }
    push_field(
        &mut markdown,
        "Source audio",
        &format!(
            "{} Hz, {} channel(s)",
            export.session.sample_rate_hz, export.session.channels
        ),
    );
    push_field(
        &mut markdown,
        "Chunk duration",
        &format!("{} ms", export.session.chunk_duration_ms),
    );
    push_field(&mut markdown, "Engine", &export.session.engine);
    push_field(&mut markdown, "Model", &export.session.model);
    push_field(
        &mut markdown,
        "Segment count",
        &export.session.segment_count.to_string(),
    );
    push_field(
        &mut markdown,
        "Segmenting",
        &format!(
            "{}; vad_available={}",
            export.segmenting.strategy, export.segmenting.vad_available
        ),
    );
    if let Some(level) = &export.audio_level {
        push_field(
            &mut markdown,
            "Audio level",
            &format!(
                "mean={:.1} dBFS; peak={:.1} dBFS; near_silent={}",
                level.mean_dbfs, level.peak_dbfs, level.near_silent
            ),
        );
    }
    push_field(
        &mut markdown,
        "Inactivity auto-stop",
        &format!(
            "enabled={}; {}",
            export.session.inactivity_auto_stop.enabled, export.session.inactivity_auto_stop.reason
        ),
    );
    markdown.push('\n');

    markdown.push_str("## Retention Policy\n\n");
    push_field(
        &mut markdown,
        "Metadata",
        &export.retention.metadata.to_string(),
    );
    push_field(
        &mut markdown,
        "Transcripts",
        &export.retention.transcripts.to_string(),
    );
    push_field(&mut markdown, "Audio", &export.retention.audio.to_string());
    markdown.push('\n');

    markdown.push_str("## Artifacts\n\n");
    push_field(
        &mut markdown,
        "Segments JSONL",
        &export.artifacts.segments_jsonl,
    );
    push_field(&mut markdown, "JSON export", &export.artifacts.json_export);
    push_field(
        &mut markdown,
        "Markdown export",
        &export.artifacts.markdown_export,
    );
    if let Some(chunks_dir) = &export.artifacts.chunks_dir {
        push_field(&mut markdown, "Audio chunks", chunks_dir);
    }
    markdown.push('\n');

    markdown.push_str("## Final Transcript\n\n");
    markdown.push_str(export.final_text.as_deref().unwrap_or("<not retained>"));

    if !export.warnings.is_empty() {
        markdown.push_str("\n\n## Warnings\n\n");
        for warning in &export.warnings {
            markdown.push_str(&format!("- {warning}\n"));
        }
    }

    markdown.push_str("\n\n## Segments\n\n");
    if export.segments.is_empty() {
        markdown.push_str("<none>\n");
    } else {
        for segment in &export.segments {
            let text = segment.text.as_deref().unwrap_or("<not retained>");
            markdown.push_str(&format!(
                "- [{} - {}] {}: {}\n",
                format_offset(segment.start_ms),
                format_offset(segment.end_ms),
                segment.source_label.as_str(),
                text
            ));
        }
    }

    markdown.trim_end().to_string()
}

fn push_field(markdown: &mut String, label: &str, value: &str) {
    markdown.push_str(&format!("- **{label}:** {value}\n"));
}

fn format_offset(ms: u64) -> String {
    let hours = ms / 3_600_000;
    let minutes = (ms % 3_600_000) / 60_000;
    let seconds = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

#[derive(Debug, Serialize)]
struct JsonlSessionRecord<'a> {
    record_type: &'static str,
    schema_version: &'a str,
    session: &'a MeetingExportSession,
    source: &'a MeetingSourceMetadata,
    retention: &'a MeetingRetentionPolicy,
    segmenting: &'a MeetingSegmenting,
    artifacts: &'a MeetingArtifacts,
}

#[derive(Debug, Serialize)]
struct JsonlSegmentRecord<'a> {
    record_type: &'static str,
    schema_version: &'a str,
    session_id: &'a str,
    #[serde(flatten)]
    segment: &'a MeetingSegmentExport,
}

pub struct FileMeetingStore {
    root: PathBuf,
}

impl FileMeetingStore {
    pub fn new(paths: &ConfigPaths) -> Self {
        Self {
            root: paths.data_dir.join("meetings"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn paths_for_new_session(&self) -> NewMeetingPaths {
        let session_id = output::new_session_id();
        let session_dir = self.root.join(&session_id);
        NewMeetingPaths {
            session_id,
            session_dir: session_dir.clone(),
            chunks_dir: session_dir.join("chunks"),
            recorder_stderr_path: session_dir.join("capture.stderr"),
            segments_jsonl_path: session_dir.join("segments.jsonl"),
            json_export_path: session_dir.join("transcript.json"),
            markdown_export_path: session_dir.join("transcript.md"),
        }
    }

    pub fn create_session(&self, session: &MeetingSessionState) -> Result<(), ComlinkError> {
        fs::create_dir_all(Path::new(&session.chunks_dir))?;
        for stream in &session.source.streams {
            if let Some(chunks_dir) = &stream.chunks_dir {
                fs::create_dir_all(chunks_dir)?;
            }
        }
        self.save_session(session)?;
        fs::write(self.active_file(), &session.session_id)?;
        Ok(())
    }

    pub fn save_session(&self, session: &MeetingSessionState) -> Result<(), ComlinkError> {
        fs::create_dir_all(Path::new(&session.session_dir))?;
        let bytes = serde_json::to_vec_pretty(session)?;
        fs::write(self.session_file(&session.session_id), bytes)?;
        Ok(())
    }

    pub fn read_session(&self, id: &str) -> Result<MeetingSessionState, ComlinkError> {
        let path = self.session_file(id);
        if !path.is_file() {
            return Err(ComlinkError::MeetingSessionNotFound(id.to_string()));
        }
        serde_json::from_slice(&fs::read(path)?).map_err(ComlinkError::from)
    }

    pub fn active_session_id(&self) -> Result<Option<String>, ComlinkError> {
        let path = self.active_file();
        if !path.is_file() {
            return Ok(None);
        }
        let id = fs::read_to_string(path)?.trim().to_string();
        Ok((!id.is_empty()).then_some(id))
    }

    pub fn clear_active_if_matches(&self, id: &str) -> Result<(), ComlinkError> {
        if self.active_session_id()?.as_deref() == Some(id) {
            match fs::remove_file(self.active_file()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub fn latest_stopped_session_id(&self) -> Result<Option<String>, ComlinkError> {
        if !self.root.is_dir() {
            return Ok(None);
        }

        let mut candidates = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            if let Ok(session) = self.read_session(&id) {
                if session.status == MeetingStatus::Stopped {
                    candidates.push((session.started_at_ms, session.session_id));
                }
            }
        }
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
        Ok(candidates.into_iter().map(|(_, id)| id).next())
    }

    pub fn discover_chunks(
        &self,
        session: &MeetingSessionState,
        duration_probe: impl Fn(&Path) -> Option<u64>,
    ) -> Result<Vec<MeetingChunk>, ComlinkError> {
        self.discover_source_chunks(session, duration_probe)
    }

    pub fn discover_source_chunks(
        &self,
        session: &MeetingSessionState,
        duration_probe: impl Fn(&Path) -> Option<u64>,
    ) -> Result<Vec<MeetingChunk>, ComlinkError> {
        let mut chunks = Vec::new();
        for stream in &session.source.streams {
            let stream_chunks_dir = stream
                .chunks_dir
                .as_deref()
                .unwrap_or(session.chunks_dir.as_str());
            chunks.extend(discover_chunks_in_dir(
                Path::new(stream_chunks_dir),
                stream.label,
                &stream.device,
                session.chunk_duration_ms,
                &duration_probe,
            )?);
        }
        chunks.sort_by(|left, right| {
            left.start_ms
                .cmp(&right.start_ms)
                .then_with(|| left.source_label.as_str().cmp(right.source_label.as_str()))
                .then_with(|| left.path.cmp(&right.path))
        });
        for (index, chunk) in chunks.iter_mut().enumerate() {
            chunk.index = index;
        }
        Ok(chunks)
    }

    pub fn discover_chunks_for_stream(
        &self,
        session: &MeetingSessionState,
        stream: &MeetingSourceStream,
        duration_probe: impl Fn(&Path) -> Option<u64>,
    ) -> Result<Vec<MeetingChunk>, ComlinkError> {
        let stream_chunks_dir = stream
            .chunks_dir
            .as_deref()
            .unwrap_or(session.chunks_dir.as_str());
        discover_chunks_in_dir(
            Path::new(stream_chunks_dir),
            stream.label,
            &stream.device,
            session.chunk_duration_ms,
            duration_probe,
        )
    }

    pub fn delete_unretained_chunks(
        &self,
        session: &MeetingSessionState,
    ) -> Result<(), ComlinkError> {
        let chunks_dir = Path::new(&session.chunks_dir);
        if chunks_dir.exists() {
            fs::remove_dir_all(chunks_dir)?;
        }
        Ok(())
    }

    pub fn write_segments_jsonl(&self, export: &MeetingExport) -> Result<(), ComlinkError> {
        let path = Path::new(&export.artifacts.segments_jsonl);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(path)?;
        let record = JsonlSessionRecord {
            record_type: "session",
            schema_version: &export.schema_version,
            session: &export.session,
            source: &export.source,
            retention: &export.retention,
            segmenting: &export.segmenting,
            artifacts: &export.artifacts,
        };
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;

        for segment in &export.segments {
            let record = JsonlSegmentRecord {
                record_type: "segment",
                schema_version: &export.schema_version,
                session_id: &export.session.session_id,
                segment,
            };
            serde_json::to_writer(&mut file, &record)?;
            file.write_all(b"\n")?;
        }

        Ok(())
    }

    pub fn append_segment_jsonl(
        &self,
        session: &MeetingSessionState,
        segment: &MeetingSegmentExport,
    ) -> Result<(), ComlinkError> {
        let path = Path::new(&session.segments_jsonl_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let record = JsonlSegmentRecord {
            record_type: "segment",
            schema_version: MEETING_SCHEMA_VERSION,
            session_id: &session.session_id,
            segment,
        };
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    pub fn write_exports(&self, export: &MeetingExport) -> Result<(), ComlinkError> {
        fs::write(
            &export.artifacts.json_export,
            serde_json::to_vec_pretty(export)?,
        )?;
        fs::write(&export.artifacts.markdown_export, render_markdown(export))?;
        Ok(())
    }

    pub fn read_export(&self, id: &str, kind: MeetingExportKind) -> Result<String, ComlinkError> {
        let session = self.read_session(id)?;
        if session.status != MeetingStatus::Stopped {
            return Err(ComlinkError::MeetingNotStopped(id.to_string()));
        }
        let path = match kind {
            MeetingExportKind::Json => PathBuf::from(&session.json_export_path),
            MeetingExportKind::Markdown => PathBuf::from(&session.markdown_export_path),
        };
        if !path.is_file() {
            return Err(ComlinkError::MeetingExportUnavailable(path));
        }
        fs::read_to_string(path).map_err(ComlinkError::from)
    }

    pub fn delete_chunks(&self, session: &MeetingSessionState) -> Result<(), ComlinkError> {
        self.delete_unretained_chunks(session)
    }

    fn active_file(&self) -> PathBuf {
        self.root.join(ACTIVE_SESSION_FILE)
    }

    fn session_file(&self, id: &str) -> PathBuf {
        self.root.join(id).join("session.json")
    }
}

fn discover_chunks_in_dir(
    chunks_dir: &Path,
    source_label: MeetingSourceLabel,
    source_device: &str,
    chunk_duration_ms: u64,
    duration_probe: impl Fn(&Path) -> Option<u64>,
) -> Result<Vec<MeetingChunk>, ComlinkError> {
    if !chunks_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = fs::read_dir(chunks_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("wav"))
        .collect::<Vec<_>>();
    paths.sort();

    let mut start_ms = 0;
    let mut chunks = Vec::with_capacity(paths.len());
    for (index, path) in paths.into_iter().enumerate() {
        let duration_ms = duration_probe(&path)
            .filter(|duration| *duration > 0)
            .unwrap_or(chunk_duration_ms);
        chunks.push(MeetingChunk {
            index,
            path,
            source_label,
            source_device: source_device.to_string(),
            start_ms,
            duration_ms,
        });
        start_ms = start_ms.saturating_add(duration_ms);
    }

    Ok(chunks)
}

#[derive(Debug, Clone)]
pub struct NewMeetingPaths {
    pub session_id: String,
    pub session_dir: PathBuf,
    pub chunks_dir: PathBuf,
    pub recorder_stderr_path: PathBuf,
    pub segments_jsonl_path: PathBuf,
    pub json_export_path: PathBuf,
    pub markdown_export_path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub enum MeetingExportKind {
    Json,
    Markdown,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::{
        asr::{Segment, SourceMetadata},
        config::RetentionConfig,
        output::ProcessingStep,
    };

    use super::*;

    fn transcript(text: &str, duration_ms: u64, segments: Vec<Segment>) -> Transcript {
        Transcript {
            text: text.to_string(),
            engine: "whisper.cpp".to_string(),
            model: "model.bin".to_string(),
            duration_ms,
            segments,
            source: SourceMetadata {
                path: "chunk.wav".to_string(),
                normalized_sample_rate_hz: 16_000,
                normalized_channels: 1,
            },
        }
    }

    #[test]
    fn chunk_transcripts_stitch_into_monotonic_meeting_segments() {
        let chunks = vec![
            ChunkTranscript {
                chunk_index: 0,
                chunk_path: PathBuf::from("chunk-00000.wav"),
                source_label: MeetingSourceLabel::UserMic,
                source_device: ":0".to_string(),
                chunk_start_ms: 0,
                chunk_duration_ms: 30_000,
                transcript: transcript("alpha", 30_000, Vec::new()),
            },
            ChunkTranscript {
                chunk_index: 1,
                chunk_path: PathBuf::from("chunk-00001.wav"),
                source_label: MeetingSourceLabel::UserMic,
                source_device: ":0".to_string(),
                chunk_start_ms: 30_000,
                chunk_duration_ms: 30_000,
                transcript: transcript("bravo", 30_000, Vec::new()),
            },
        ];

        let result = build_segments_from_chunk_transcripts(&chunks);

        assert_eq!(result.segments.len(), 2);
        assert_eq!(result.segments[0].start_ms, 0);
        assert_eq!(result.segments[0].end_ms, 30_000);
        assert_eq!(result.segments[1].start_ms, 30_000);
        assert_eq!(result.segments[1].end_ms, 60_000);
        assert_eq!(raw_text_from_segments(&result.segments), "alpha\nbravo");
        assert!(!result.segmenting.vad_available);
    }

    #[test]
    fn asr_sub_segments_are_offset_into_meeting_time() {
        let chunks = vec![ChunkTranscript {
            chunk_index: 2,
            chunk_path: PathBuf::from("chunk-00002.wav"),
            source_label: MeetingSourceLabel::UserMic,
            source_device: ":0".to_string(),
            chunk_start_ms: 60_000,
            chunk_duration_ms: 30_000,
            transcript: transcript(
                "alpha bravo",
                30_000,
                vec![
                    Segment {
                        start_ms: 1_000,
                        end_ms: 6_000,
                        text: "alpha".to_string(),
                    },
                    Segment {
                        start_ms: 8_000,
                        end_ms: 12_000,
                        text: "bravo".to_string(),
                    },
                ],
            ),
        }];

        let result = build_segments_from_chunk_transcripts(&chunks);

        assert_eq!(result.segments[0].start_ms, 61_000);
        assert_eq!(result.segments[0].end_ms, 66_000);
        assert_eq!(result.segments[1].start_ms, 68_000);
        assert_eq!(result.segments[1].end_ms, 72_000);
        assert_eq!(result.segmenting.strategy, "asr-segments-offset-by-chunk");
        assert!(result.segmenting.vad_available);
    }

    #[test]
    fn export_applies_retention_without_dropping_policy_metadata() {
        let retention = MeetingRetentionPolicy::from(&RetentionConfig {
            metadata: false,
            transcripts: false,
            audio: false,
        });
        let mut session = new_recording_session(NewMeetingSession {
            mode: "raw".to_string(),
            session_id: "meeting-1".to_string(),
            no_llm: true,
            device: ":0".to_string(),
            source: MeetingSourceMetadata::default(),
            chunk_duration_ms: 30_000,
            model: "/models/ggml.bin".to_string(),
            model_path: "/models/ggml.bin".to_string(),
            retention,
            session_dir: PathBuf::from("/tmp/session"),
            chunks_dir: PathBuf::from("/tmp/session/chunks"),
            recorder_stderr_path: PathBuf::from("/tmp/session/capture.stderr"),
            segments_jsonl_path: PathBuf::from("/tmp/session/segments.jsonl"),
            json_export_path: PathBuf::from("/tmp/session/transcript.json"),
            markdown_export_path: PathBuf::from("/tmp/session/transcript.md"),
        });
        session.mark_stopped(200, 100, 1);
        let processed = TextProcessingResult {
            mode: "raw".to_string(),
            final_text: "secret".to_string(),
            processing_steps: vec![ProcessingStep {
                name: "raw".to_string(),
            }],
            warnings: Vec::new(),
            llm: None,
        };
        let segments = vec![MeetingSegment {
            segment_index: 0,
            chunk_index: 0,
            start_ms: 0,
            end_ms: 100,
            source_label: MeetingSourceLabel::UserMic,
            source_device: ":0".to_string(),
            text: "secret".to_string(),
            chunk_path: "/tmp/session/chunks/chunk-00000.wav".to_string(),
        }];

        let export = build_export(
            &session,
            &segments,
            "secret",
            &processed,
            MeetingSegmenting {
                strategy: "chunk-boundaries".to_string(),
                vad_available: false,
                detail: "test".to_string(),
            },
            None,
        );

        assert_eq!(export.session.source, "<redacted>");
        assert_eq!(export.session.model, "<redacted>");
        assert!(!export.retention.transcripts);
        assert_eq!(export.final_text, None);
        assert_eq!(export.segments[0].text, None);
        assert_eq!(export.segments[0].chunk_path, None);
    }

    #[test]
    fn export_warns_about_noisy_tiny_model_meeting_transcripts() {
        let retention = MeetingRetentionPolicy::from(&RetentionConfig {
            metadata: true,
            transcripts: true,
            audio: false,
        });
        let mut session = new_recording_session(NewMeetingSession {
            mode: "raw".to_string(),
            session_id: "meeting-1".to_string(),
            no_llm: true,
            device: ":0".to_string(),
            source: MeetingSourceMetadata::default(),
            chunk_duration_ms: 30_000,
            model: "/models/ggml-tiny.en.bin".to_string(),
            model_path: "/models/ggml-tiny.en.bin".to_string(),
            retention,
            session_dir: PathBuf::from("/tmp/session"),
            chunks_dir: PathBuf::from("/tmp/session/chunks"),
            recorder_stderr_path: PathBuf::from("/tmp/session/capture.stderr"),
            segments_jsonl_path: PathBuf::from("/tmp/session/segments.jsonl"),
            json_export_path: PathBuf::from("/tmp/session/transcript.json"),
            markdown_export_path: PathBuf::from("/tmp/session/transcript.md"),
        });
        session.mark_stopped(200, 180_000, 1);
        let repeated = "[BLANK_AUDIO] Please go back to the phone. \
            Please go back to the phone. Please go back to the phone. \
            Please go back to the phone. Please go back to the phone. \
            Please go back to the phone.";
        let processed = TextProcessingResult {
            mode: "raw".to_string(),
            final_text: repeated.to_string(),
            processing_steps: vec![ProcessingStep {
                name: "raw".to_string(),
            }],
            warnings: Vec::new(),
            llm: None,
        };
        let segments = vec![MeetingSegment {
            segment_index: 0,
            chunk_index: 0,
            start_ms: 0,
            end_ms: 180_000,
            source_label: MeetingSourceLabel::UserMic,
            source_device: ":0".to_string(),
            text: repeated.to_string(),
            chunk_path: "/tmp/session/chunks/chunk-00000.wav".to_string(),
        }];

        let export = build_export(
            &session,
            &segments,
            repeated,
            &processed,
            MeetingSegmenting {
                strategy: "chunk-boundaries".to_string(),
                vad_available: false,
                detail: "test".to_string(),
            },
            None,
        );

        assert_eq!(export.warnings.len(), 4);
        assert!(export
            .warnings
            .iter()
            .any(|warning| warning.contains("tiny Whisper model")));
        assert!(export
            .warnings
            .iter()
            .any(|warning| warning.contains("[BLANK_AUDIO]")));
        assert!(export
            .warnings
            .iter()
            .any(|warning| warning.contains("repeated phrase loop")));
        assert!(export
            .warnings
            .iter()
            .any(|warning| warning.contains("raw_text preserves")));
        assert!(export
            .processing_steps
            .contains(&"meeting-repetition-loop-collapse".to_string()));
        assert_eq!(export.raw_text.as_deref(), Some(repeated));
        assert!(export
            .final_text
            .as_deref()
            .unwrap()
            .contains("[repeated phrase loop collapsed]"));
        assert!(export.segments[0]
            .text
            .as_deref()
            .unwrap()
            .contains("[repeated phrase loop collapsed]"));
    }

    #[test]
    fn export_warns_and_records_structured_field_for_near_silent_capture() {
        let retention = MeetingRetentionPolicy::from(&RetentionConfig {
            metadata: true,
            transcripts: true,
            audio: false,
        });
        let mut session = new_recording_session(NewMeetingSession {
            mode: "raw".to_string(),
            session_id: "meeting-silent".to_string(),
            no_llm: true,
            device: ":0".to_string(),
            source: MeetingSourceMetadata::default(),
            chunk_duration_ms: 30_000,
            model: "/models/ggml-base.en.bin".to_string(),
            model_path: "/models/ggml-base.en.bin".to_string(),
            retention,
            session_dir: PathBuf::from("/tmp/session"),
            chunks_dir: PathBuf::from("/tmp/session/chunks"),
            recorder_stderr_path: PathBuf::from("/tmp/session/capture.stderr"),
            segments_jsonl_path: PathBuf::from("/tmp/session/segments.jsonl"),
            json_export_path: PathBuf::from("/tmp/session/transcript.json"),
            markdown_export_path: PathBuf::from("/tmp/session/transcript.md"),
        });
        session.mark_stopped(200, 60_000, 1);
        let processed = TextProcessingResult {
            mode: "raw".to_string(),
            final_text: "you you you".to_string(),
            processing_steps: vec![ProcessingStep {
                name: "raw".to_string(),
            }],
            warnings: Vec::new(),
            llm: None,
        };
        let segments = vec![MeetingSegment {
            segment_index: 0,
            chunk_index: 0,
            start_ms: 0,
            end_ms: 60_000,
            source_label: MeetingSourceLabel::UserMic,
            source_device: ":0".to_string(),
            text: "you you you".to_string(),
            chunk_path: "/tmp/session/chunks/chunk-00000.wav".to_string(),
        }];

        let audio_level = Some(MeetingAudioLevel {
            mean_dbfs: -72.0,
            peak_dbfs: -60.0,
            near_silent: true,
        });
        let export = build_export(
            &session,
            &segments,
            "you you you",
            &processed,
            MeetingSegmenting {
                strategy: "chunk-boundaries".to_string(),
                vad_available: false,
                detail: "test".to_string(),
            },
            audio_level,
        );

        assert_eq!(export.audio_level, audio_level);
        assert!(export
            .warnings
            .iter()
            .any(|warning| warning.contains("near-silent") && warning.contains("-72.0 dBFS")));

        // A loud session carries the structured field but no near-silent warning.
        let loud = build_export(
            &session,
            &segments,
            "you you you",
            &processed,
            MeetingSegmenting {
                strategy: "chunk-boundaries".to_string(),
                vad_available: false,
                detail: "test".to_string(),
            },
            Some(MeetingAudioLevel {
                mean_dbfs: -28.0,
                peak_dbfs: -6.0,
                near_silent: false,
            }),
        );
        assert!(!loud
            .warnings
            .iter()
            .any(|warning| warning.contains("near-silent")));
    }

    #[test]
    fn repetition_loop_collapse_handles_sentence_and_clause_runs() {
        let text = "Start. I need your help. I need your help. I need your help. \
            I need your help. I need your help. I need your help. Then it is like, \
            it is like, it is like, it is like, it is like, it is like, finished.";

        let (collapsed, changed) = collapse_repetition_loops(text);

        assert!(changed);
        assert!(collapsed.contains("Start."));
        assert!(collapsed.contains("finished."));
        assert_eq!(collapsed.matches("I need your help.").count(), 2);
        assert!(collapsed.contains("Then it is like, it is like, it is like,"));
        assert_eq!(
            collapsed
                .matches("[repeated phrase loop collapsed]")
                .count(),
            2
        );
    }

    #[test]
    fn repetition_loop_collapse_leaves_short_emphasis_alone() {
        let text = "No, no, no, no, no. Yeah. Yeah. Thanks.";

        let (collapsed, changed) = collapse_repetition_loops(text);

        assert!(!changed);
        assert_eq!(collapsed, text);
    }

    #[test]
    fn repetition_loop_detects_repeated_single_token_whisper_hallucination() {
        // The canonical whisper-on-silence artifact: one short token repeated.
        // The phrase-run check misses it (single tokens are < 12 chars), so the
        // unigram path must catch it.
        let text = "you you you you you you you you you you you you";
        assert!(has_repetition_loop(text));
        assert!(!has_phrase_repetition_loop(text));
        assert!(has_dominant_unigram_loop(text));
    }

    #[test]
    fn repetition_loop_detects_repeated_token_across_sentences() {
        // whisper often punctuates the filler ("Thank you. Thank you. ...").
        let text = "Thank you. ".repeat(12);
        assert!(has_repetition_loop(&text));
    }

    #[test]
    fn repetition_loop_ignores_short_emphatic_repeats() {
        // Real emphatic speech repeats a token a handful of times: must not trip.
        assert!(!has_repetition_loop("No, no, no, no, no. Yeah. Thanks."));
        assert!(!has_repetition_loop(
            "We shipped the feature and the team is happy with the result."
        ));
        assert!(!has_repetition_loop("yes yes yes")); // short run, below threshold
    }

    #[test]
    fn repetition_loop_dominance_needs_a_long_transcript() {
        // A single token dominating a long transcript trips the dominance path
        // even without a consecutive run long enough to hit UNIGRAM_RUN_THRESHOLD.
        // "you" is 14/21 (~67%) of >= 20 tokens, with a max run of 2.
        let dominated = "you you and ".repeat(7);
        assert!(has_dominant_unigram_loop(&dominated));

        // The same ratio in a short transcript stays quiet.
        assert!(!has_dominant_unigram_loop("you and you but you"));
    }

    #[test]
    fn export_warns_on_degenerate_unigram_transcript_without_audio_level() {
        // AC: the `you you you...` repro triggers a transcript warning even when
        // the audio-level path is unavailable (audio_level = None).
        let session = new_recording_session(NewMeetingSession {
            mode: "raw".to_string(),
            session_id: "meeting-unigram".to_string(),
            no_llm: true,
            device: ":0".to_string(),
            source: MeetingSourceMetadata::default(),
            chunk_duration_ms: 30_000,
            model: "/models/ggml-base.en.bin".to_string(),
            model_path: "/models/ggml-base.en.bin".to_string(),
            retention: MeetingRetentionPolicy::from(&RetentionConfig {
                metadata: true,
                transcripts: true,
                audio: false,
            }),
            session_dir: PathBuf::from("/tmp/session"),
            chunks_dir: PathBuf::from("/tmp/session/chunks"),
            recorder_stderr_path: PathBuf::from("/tmp/session/capture.stderr"),
            segments_jsonl_path: PathBuf::from("/tmp/session/segments.jsonl"),
            json_export_path: PathBuf::from("/tmp/session/transcript.json"),
            markdown_export_path: PathBuf::from("/tmp/session/transcript.md"),
        });
        let text = "you you you you you you you you you you you you";
        let processed = TextProcessingResult {
            mode: "raw".to_string(),
            final_text: text.to_string(),
            processing_steps: vec![ProcessingStep {
                name: "raw".to_string(),
            }],
            warnings: Vec::new(),
            llm: None,
        };
        let segments = vec![MeetingSegment {
            segment_index: 0,
            chunk_index: 0,
            start_ms: 0,
            end_ms: 60_000,
            source_label: MeetingSourceLabel::UserMic,
            source_device: ":0".to_string(),
            text: text.to_string(),
            chunk_path: "/tmp/session/chunks/chunk-00000.wav".to_string(),
        }];

        let export = build_export(
            &session,
            &segments,
            text,
            &processed,
            MeetingSegmenting {
                strategy: "chunk-boundaries".to_string(),
                vad_available: false,
                detail: "test".to_string(),
            },
            None,
        );

        assert!(export
            .warnings
            .iter()
            .any(|warning| warning.contains("repeated phrase loop")));
    }
}
