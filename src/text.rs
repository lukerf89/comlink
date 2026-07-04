use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum TextMode {
    Raw,
    Memo,
}

impl TextMode {
    pub fn process(self, raw_text: &str) -> String {
        match self {
            Self::Raw => raw_text.trim().to_string(),
            Self::Memo => clean_memo(raw_text),
        }
    }

    pub fn processing_step(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Memo => "memo-cleanup",
        }
    }
}

fn clean_memo(raw_text: &str) -> String {
    let collapsed = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut cleaned = String::with_capacity(collapsed.len());

    for ch in collapsed.chars() {
        if matches!(ch, '.' | ',' | ';' | ':' | '!' | '?') && cleaned.ends_with(' ') {
            cleaned.pop();
        }
        cleaned.push(ch);
    }

    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_cleanup_collapses_whitespace_and_punctuation_spacing() {
        assert_eq!(
            TextMode::Memo.process("  first line \n second line  .  "),
            "first line second line."
        );
    }

    #[test]
    fn raw_cleanup_only_trims_edges() {
        assert_eq!(
            TextMode::Raw.process("  first   second  "),
            "first   second"
        );
    }
}
