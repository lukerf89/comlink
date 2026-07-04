use clap::ValueEnum;

use crate::{asr::Transcript, error::ComlinkError};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

pub fn print_transcript(transcript: &Transcript, format: OutputFormat) -> Result<(), ComlinkError> {
    match format {
        OutputFormat::Text => {
            println!("{}", transcript.text);
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
            text: "hello".to_string(),
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

        let json = serde_json::to_value(&transcript).unwrap();
        assert_eq!(json["text"], "hello");
        assert_eq!(json["engine"], "whisper.cpp");
        assert_eq!(json["model"], "model.bin");
        assert_eq!(json["duration_ms"], 500);
        assert!(json["segments"].is_array());
        assert_eq!(json["source"]["normalized_sample_rate_hz"], 16_000);
    }
}
