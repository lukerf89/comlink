use std::{
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::ValueEnum;
use serde::Serialize;

use crate::{
    asr::{Segment, SourceMetadata, Transcript},
    config::Config,
    error::ComlinkError,
    llm::{self, LlmRewriteRecord, RewriteInput},
    text::{self, TextRules},
};

pub const SCHEMA_VERSION: &str = "comlink.session.v1";
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    Jsonl,
    Md,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessingStep {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextMetadata {
    pub policy: String,
    pub items: Vec<String>,
}

impl Default for ContextMetadata {
    fn default() -> Self {
        Self {
            policy: "none".to_string(),
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptOutput {
    pub schema_version: String,
    pub session_id: String,
    pub text: String,
    pub raw_text: String,
    pub final_text: String,
    pub mode: String,
    pub copied: bool,
    pub engine: String,
    pub model: String,
    pub duration_ms: u64,
    pub segments: Vec<Segment>,
    pub source: SourceMetadata,
    pub context: ContextMetadata,
    pub processing_steps: Vec<ProcessingStep>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmRewriteRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_session_id: Option<String>,
}

impl TranscriptOutput {
    pub fn from_transcript(
        transcript: Transcript,
        mode: &str,
        copied: bool,
        config: &Config,
        no_llm: bool,
    ) -> Result<Self, ComlinkError> {
        let raw_text = transcript.text;
        let processed = process_text(&raw_text, mode, config, no_llm)?;

        Ok(Self {
            schema_version: SCHEMA_VERSION.to_string(),
            session_id: new_session_id(),
            text: processed.final_text.clone(),
            raw_text,
            final_text: processed.final_text,
            mode: processed.mode,
            copied,
            engine: transcript.engine,
            model: transcript.model,
            duration_ms: transcript.duration_ms,
            segments: transcript.segments,
            source: transcript.source,
            context: ContextMetadata::default(),
            processing_steps: processed.processing_steps,
            warnings: processed.warnings,
            llm: processed.llm,
            history_session_id: None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct TextProcessingResult {
    pub mode: String,
    pub final_text: String,
    pub processing_steps: Vec<ProcessingStep>,
    pub warnings: Vec<String>,
    pub llm: Option<LlmRewriteRecord>,
}

pub fn process_text(
    raw_text: &str,
    mode: &str,
    config: &Config,
    no_llm: bool,
) -> Result<TextProcessingResult, ComlinkError> {
    let mode = text::resolve_mode(config, mode)
        .ok_or_else(|| ComlinkError::ModeNotFound(mode.to_string()))?;
    let deterministic_text = mode.process_deterministic(
        raw_text,
        TextRules {
            vocabulary: &config.vocabulary,
            snippets: &config.snippets,
        },
    );
    let mut final_text = deterministic_text.clone();
    let mut warnings = Vec::new();
    let mut processing_steps = mode
        .processing_steps()
        .into_iter()
        .map(|step| ProcessingStep { name: step })
        .collect::<Vec<_>>();
    let mut llm_record = None;

    if let Some(instruction) = mode.llm_instruction.as_deref() {
        if no_llm {
            llm_record = Some(llm::skipped_record("--no-llm was set"));
        } else {
            match llm::rewrite(
                &config.llm,
                RewriteInput {
                    mode: &mode.name,
                    text: &deterministic_text,
                    instruction,
                    profile: mode.style_profile,
                },
            ) {
                Ok(success) => {
                    final_text = success.text;
                    processing_steps.push(ProcessingStep {
                        name: "llm-rewrite".to_string(),
                    });
                    llm_record = Some(success.record);
                }
                Err(record) => {
                    let record = *record;
                    let warning = record
                        .error
                        .clone()
                        .unwrap_or_else(|| "local LLM rewrite failed".to_string());
                    warnings.push(format!("local LLM rewrite unavailable: {warning}"));
                    llm_record = Some(record);
                }
            }
        }
    }

    Ok(TextProcessingResult {
        mode: mode.name,
        final_text,
        processing_steps,
        warnings,
        llm: llm_record,
    })
}

pub fn print_transcript(
    transcript: &TranscriptOutput,
    format: OutputFormat,
) -> Result<(), ComlinkError> {
    match format {
        OutputFormat::Text => {
            println!("{}", transcript.final_text);
            Ok(())
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(transcript)?);
            Ok(())
        }
        OutputFormat::Jsonl => print_jsonl(transcript),
        OutputFormat::Md => {
            println!("{}", render_markdown(transcript));
            Ok(())
        }
    }
}

pub fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("s{nanos}-{}-{sequence}", process::id())
}

fn print_jsonl(transcript: &TranscriptOutput) -> Result<(), ComlinkError> {
    let metadata = JsonlSessionRecord {
        record_type: "session",
        schema_version: &transcript.schema_version,
        session_id: &transcript.session_id,
        mode: &transcript.mode,
        engine: &transcript.engine,
        model: &transcript.model,
        duration_ms: transcript.duration_ms,
        source: &transcript.source,
        context: &transcript.context,
        copied: transcript.copied,
        history_session_id: transcript.history_session_id.as_deref(),
        processing_steps: &transcript.processing_steps,
        warnings: &transcript.warnings,
        llm: transcript.llm.as_ref(),
    };
    println!("{}", serde_json::to_string(&metadata)?);

    for (index, segment) in transcript.segments.iter().enumerate() {
        let record = JsonlSegmentRecord {
            record_type: "segment",
            schema_version: &transcript.schema_version,
            session_id: &transcript.session_id,
            segment_index: index,
            segment,
        };
        println!("{}", serde_json::to_string(&record)?);
    }

    let final_record = JsonlTranscriptRecord {
        record_type: "transcript",
        schema_version: &transcript.schema_version,
        session_id: &transcript.session_id,
        text: &transcript.text,
        raw_text: &transcript.raw_text,
        final_text: &transcript.final_text,
        mode: &transcript.mode,
        copied: transcript.copied,
        engine: &transcript.engine,
        model: &transcript.model,
        duration_ms: transcript.duration_ms,
        source: &transcript.source,
        context: &transcript.context,
        processing_steps: &transcript.processing_steps,
        warnings: &transcript.warnings,
        llm: transcript.llm.as_ref(),
        history_session_id: transcript.history_session_id.as_deref(),
    };
    println!("{}", serde_json::to_string(&final_record)?);
    Ok(())
}

pub fn render_markdown(transcript: &TranscriptOutput) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Comlink Transcript\n\n");
    markdown.push_str("## Metadata\n\n");
    push_field(&mut markdown, "Schema", &transcript.schema_version);
    push_field(&mut markdown, "Session", &transcript.session_id);
    if let Some(id) = &transcript.history_session_id {
        push_field(&mut markdown, "History session", id);
    }
    push_field(&mut markdown, "Mode", &transcript.mode);
    push_field(&mut markdown, "Engine", &transcript.engine);
    push_field(&mut markdown, "Model", &transcript.model);
    push_field(
        &mut markdown,
        "Duration",
        &format!("{} ms", transcript.duration_ms),
    );
    push_field(&mut markdown, "Source", &transcript.source.path);
    push_field(
        &mut markdown,
        "Source audio",
        &format!(
            "{} Hz, {} channel(s)",
            transcript.source.normalized_sample_rate_hz, transcript.source.normalized_channels
        ),
    );
    push_field(&mut markdown, "Context policy", &transcript.context.policy);
    if let Some(llm) = &transcript.llm {
        push_field(&mut markdown, "LLM rewrite", &llm.status);
    }
    markdown.push('\n');

    markdown.push_str("## Final Text\n\n");
    markdown.push_str(&transcript.final_text);
    markdown.push_str("\n\n## Raw Text\n\n");
    markdown.push_str(&transcript.raw_text);

    if !transcript.warnings.is_empty() {
        markdown.push_str("\n\n## Warnings\n\n");
        for warning in &transcript.warnings {
            markdown.push_str(&format!("- {warning}\n"));
        }
    }

    if !transcript.segments.is_empty() {
        markdown.push_str("\n\n## Segments\n\n");
        for (index, segment) in transcript.segments.iter().enumerate() {
            markdown.push_str(&format!(
                "- {}: {}-{} ms: {}\n",
                index, segment.start_ms, segment.end_ms, segment.text
            ));
        }
    }

    markdown.trim_end().to_string()
}

fn push_field(markdown: &mut String, label: &str, value: &str) {
    markdown.push_str(&format!("- **{label}:** {value}\n"));
}

#[derive(Debug, Serialize)]
struct JsonlSessionRecord<'a> {
    record_type: &'static str,
    schema_version: &'a str,
    session_id: &'a str,
    mode: &'a str,
    engine: &'a str,
    model: &'a str,
    duration_ms: u64,
    source: &'a SourceMetadata,
    context: &'a ContextMetadata,
    copied: bool,
    history_session_id: Option<&'a str>,
    processing_steps: &'a [ProcessingStep],
    warnings: &'a [String],
    llm: Option<&'a LlmRewriteRecord>,
}

#[derive(Debug, Serialize)]
struct JsonlSegmentRecord<'a> {
    record_type: &'static str,
    schema_version: &'a str,
    session_id: &'a str,
    segment_index: usize,
    segment: &'a Segment,
}

#[derive(Debug, Serialize)]
struct JsonlTranscriptRecord<'a> {
    record_type: &'static str,
    schema_version: &'a str,
    session_id: &'a str,
    text: &'a str,
    raw_text: &'a str,
    final_text: &'a str,
    mode: &'a str,
    copied: bool,
    engine: &'a str,
    model: &'a str,
    duration_ms: u64,
    source: &'a SourceMetadata,
    context: &'a ContextMetadata,
    processing_steps: &'a [ProcessingStep],
    warnings: &'a [String],
    llm: Option<&'a LlmRewriteRecord>,
    history_session_id: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use crate::{
        asr::{Segment, SourceMetadata},
        text::TextMode,
    };

    use super::*;

    #[test]
    fn json_includes_phase_zero_contract_fields() {
        let transcript = Transcript {
            text: "hello  .".to_string(),
            engine: "whisper.cpp".to_string(),
            model: "model.bin".to_string(),
            duration_ms: 500,
            segments: vec![Segment {
                start_ms: 0,
                end_ms: 500,
                text: "hello".to_string(),
            }],
            source: SourceMetadata {
                path: "short.wav".to_string(),
                normalized_sample_rate_hz: 16_000,
                normalized_channels: 1,
            },
        };
        let transcript = TranscriptOutput::from_transcript(
            transcript,
            TextMode::Memo.as_str(),
            true,
            &Default::default(),
            false,
        )
        .unwrap();

        let json = serde_json::to_value(&transcript).unwrap();
        assert_eq!(json["schema_version"], SCHEMA_VERSION);
        assert!(json["session_id"].as_str().unwrap().starts_with('s'));
        assert_eq!(json["text"], "hello.");
        assert_eq!(json["raw_text"], "hello  .");
        assert_eq!(json["final_text"], "hello.");
        assert_eq!(json["mode"], "memo");
        assert_eq!(json["copied"], true);
        assert_eq!(json["engine"], "whisper.cpp");
        assert_eq!(json["model"], "model.bin");
        assert_eq!(json["duration_ms"], 500);
        assert!(json["segments"].is_array());
        assert_eq!(json["source"]["normalized_sample_rate_hz"], 16_000);
        assert_eq!(json["context"]["policy"], "none");
        assert_eq!(json["context"]["items"].as_array().unwrap().len(), 0);
        assert_eq!(json["processing_steps"][0]["name"], "deterministic-cleanup");
        assert_eq!(json["processing_steps"][3]["name"], "memo-mode");
        assert!(json.get("history_session_id").is_none());
    }

    #[test]
    fn markdown_export_includes_contract_metadata_and_text() {
        let transcript = TranscriptOutput::from_transcript(
            Transcript {
                text: "raw words".to_string(),
                engine: "whisper.cpp".to_string(),
                model: "model.bin".to_string(),
                duration_ms: 100,
                segments: Vec::new(),
                source: SourceMetadata {
                    path: "short.wav".to_string(),
                    normalized_sample_rate_hz: 16_000,
                    normalized_channels: 1,
                },
            },
            TextMode::Raw.as_str(),
            false,
            &Default::default(),
            false,
        )
        .unwrap();

        let markdown = render_markdown(&transcript);

        assert!(markdown.contains("# Comlink Transcript"));
        assert!(markdown.contains("- **Schema:** comlink.session.v1"));
        assert!(markdown.contains("## Final Text"));
        assert!(markdown.contains("raw words"));
    }

    #[test]
    fn jsonl_final_record_omits_segment_array() {
        let transcript = TranscriptOutput::from_transcript(
            Transcript {
                text: "hello".to_string(),
                engine: "whisper.cpp".to_string(),
                model: "model.bin".to_string(),
                duration_ms: 100,
                segments: vec![Segment {
                    start_ms: 0,
                    end_ms: 100,
                    text: "hello".to_string(),
                }],
                source: SourceMetadata {
                    path: "short.wav".to_string(),
                    normalized_sample_rate_hz: 16_000,
                    normalized_channels: 1,
                },
            },
            TextMode::Raw.as_str(),
            false,
            &Default::default(),
            false,
        )
        .unwrap();

        let record = JsonlTranscriptRecord {
            record_type: "transcript",
            schema_version: &transcript.schema_version,
            session_id: &transcript.session_id,
            text: &transcript.text,
            raw_text: &transcript.raw_text,
            final_text: &transcript.final_text,
            mode: &transcript.mode,
            copied: transcript.copied,
            engine: &transcript.engine,
            model: &transcript.model,
            duration_ms: transcript.duration_ms,
            source: &transcript.source,
            context: &transcript.context,
            processing_steps: &transcript.processing_steps,
            warnings: &transcript.warnings,
            llm: transcript.llm.as_ref(),
            history_session_id: transcript.history_session_id.as_deref(),
        };

        let json = serde_json::to_value(record).unwrap();
        assert_eq!(json["record_type"], "transcript");
        assert_eq!(json["final_text"], "hello");
        assert!(json.get("segments").is_none());
    }
}
