use clap::ValueEnum;
use serde::Serialize;

use crate::{
    asr::{Segment, SourceMetadata, Transcript},
    config::Config,
    error::ComlinkError,
    text::{TextMode, TextRules},
};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessingStep {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptOutput {
    pub text: String,
    pub raw_text: String,
    pub final_text: String,
    pub mode: TextMode,
    pub copied: bool,
    pub engine: String,
    pub model: String,
    pub duration_ms: u64,
    pub segments: Vec<Segment>,
    pub source: SourceMetadata,
    pub processing_steps: Vec<ProcessingStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_session_id: Option<String>,
}

impl TranscriptOutput {
    pub fn from_transcript(
        transcript: Transcript,
        mode: TextMode,
        copied: bool,
        config: &Config,
    ) -> Self {
        let raw_text = transcript.text;
        let final_text = mode.process(
            &raw_text,
            TextRules {
                vocabulary: &config.vocabulary,
                snippets: &config.snippets,
            },
        );

        Self {
            text: final_text.clone(),
            raw_text,
            final_text,
            mode,
            copied,
            engine: transcript.engine,
            model: transcript.model,
            duration_ms: transcript.duration_ms,
            segments: transcript.segments,
            source: transcript.source,
            processing_steps: mode
                .processing_steps()
                .into_iter()
                .map(|step| ProcessingStep {
                    name: step.to_string(),
                })
                .collect(),
            history_session_id: None,
        }
    }
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
    }
}

#[cfg(test)]
mod tests {
    use crate::asr::{Segment, SourceMetadata};

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
            TextMode::Memo,
            true,
            &Default::default(),
        );

        let json = serde_json::to_value(&transcript).unwrap();
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
        assert_eq!(json["processing_steps"][0]["name"], "deterministic-cleanup");
        assert_eq!(json["processing_steps"][3]["name"], "memo-mode");
        assert!(json.get("history_session_id").is_none());
    }
}
