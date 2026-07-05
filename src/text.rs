use serde::Serialize;

use crate::config::{Config, ModeEntry, SnippetEntry, StyleProfile, VocabularyEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextMode {
    Raw,
    Clean,
    Memo,
    CodingPrompt,
    EmailReply,
    SlackReply,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModeDefinition {
    pub name: String,
    pub description: String,
    pub deterministic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deterministic_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style_profile: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TextRules<'a> {
    pub vocabulary: &'a [VocabularyEntry],
    pub snippets: &'a [SnippetEntry],
}

impl TextMode {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "raw" => Some(Self::Raw),
            "clean" => Some(Self::Clean),
            "memo" => Some(Self::Memo),
            "coding-prompt" => Some(Self::CodingPrompt),
            "email-reply" => Some(Self::EmailReply),
            "slack-reply" => Some(Self::SlackReply),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Clean => "clean",
            Self::Memo => "memo",
            Self::CodingPrompt => "coding-prompt",
            Self::EmailReply => "email-reply",
            Self::SlackReply => "slack-reply",
        }
    }

    pub fn process(self, raw_text: &str, rules: TextRules<'_>) -> String {
        match self {
            Self::Raw => raw_text.trim().to_string(),
            Self::Clean => apply_deterministic_rules(raw_text, rules),
            Self::Memo | Self::CodingPrompt | Self::EmailReply => {
                apply_sentence_end_mode(raw_text, rules)
            }
            Self::SlackReply => apply_slack_reply_mode(raw_text, rules),
        }
    }

    pub fn processing_steps(self) -> Vec<&'static str> {
        match self {
            Self::Raw => vec!["raw"],
            Self::Clean => vec!["deterministic-cleanup", "vocabulary", "snippets"],
            Self::Memo => vec![
                "deterministic-cleanup",
                "vocabulary",
                "snippets",
                "memo-mode",
            ],
            Self::CodingPrompt => vec![
                "deterministic-cleanup",
                "vocabulary",
                "snippets",
                "coding-prompt-mode",
            ],
            Self::EmailReply => vec![
                "deterministic-cleanup",
                "vocabulary",
                "snippets",
                "email-reply-mode",
            ],
            Self::SlackReply => vec![
                "deterministic-cleanup",
                "vocabulary",
                "snippets",
                "slack-reply-mode",
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedMode<'a> {
    pub name: String,
    pub description: String,
    pub deterministic_mode: TextMode,
    pub llm_instruction: Option<String>,
    pub style_profile: Option<&'a StyleProfile>,
    pub warnings: Vec<String>,
}

impl<'a> ResolvedMode<'a> {
    pub fn process_deterministic(&self, raw_text: &str, rules: TextRules<'_>) -> String {
        self.deterministic_mode.process(raw_text, rules)
    }

    pub fn processing_steps(&self) -> Vec<String> {
        let mut steps = self
            .deterministic_mode
            .processing_steps()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if self.name != self.deterministic_mode.as_str() {
            steps.push(format!("{}-mode", self.name));
        }
        steps
    }
}

pub fn resolve_mode<'a>(config: &'a Config, name: &str) -> Option<ResolvedMode<'a>> {
    let custom = config
        .modes
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(name));
    let builtin = TextMode::parse(name);
    let mut warnings = Vec::new();
    let deterministic_mode = if let Some(entry) = custom {
        if let Some(configured_mode) = entry.deterministic_mode.as_deref() {
            TextMode::parse(configured_mode).unwrap_or_else(|| {
                warnings.push(format!(
                    "mode '{}' references unknown deterministic mode '{}'; using {}",
                    entry.name,
                    configured_mode,
                    builtin.unwrap_or(TextMode::Memo).as_str()
                ));
                builtin.unwrap_or(TextMode::Memo)
            })
        } else {
            builtin.unwrap_or(TextMode::Memo)
        }
    } else {
        builtin?
    };
    let name = custom
        .map(|entry| entry.name.clone())
        .or_else(|| builtin.map(|mode| mode.as_str().to_string()))?;
    let description = custom
        .and_then(|entry| entry.description.clone())
        .or_else(|| builtin_description(&name).map(str::to_string))
        .unwrap_or_else(|| "Configured local work mode.".to_string());
    let configured_style_profile = custom.and_then(|entry| entry.style_profile.as_deref());
    let style_profile =
        configured_style_profile.and_then(|profile| crate::config::style_profile(config, profile));
    if let Some(profile) = configured_style_profile {
        if style_profile.is_none() {
            warnings.push(format!(
                "mode '{name}' references missing style profile '{profile}'; continuing without it"
            ));
        }
    }

    Some(ResolvedMode {
        name,
        description,
        deterministic_mode,
        llm_instruction: custom.and_then(|entry| entry.llm_instruction.clone()),
        style_profile,
        warnings,
    })
}

pub fn mode_registry(config: &Config) -> Vec<ModeDefinition> {
    let mut modes = builtin_mode_registry();
    for entry in &config.modes {
        if let Some(existing) = modes
            .iter_mut()
            .find(|mode| mode.name.eq_ignore_ascii_case(&entry.name))
        {
            merge_custom_definition(existing, entry);
        } else {
            modes.push(ModeDefinition {
                name: entry.name.clone(),
                description: entry
                    .description
                    .clone()
                    .unwrap_or_else(|| "Configured local work mode.".to_string()),
                deterministic: entry.llm_instruction.is_none(),
                deterministic_mode: entry.deterministic_mode.clone(),
                llm_instruction: entry.llm_instruction.clone(),
                style_profile: entry.style_profile.clone(),
            });
        }
    }
    modes
}

fn builtin_mode_registry() -> Vec<ModeDefinition> {
    vec![
        ModeDefinition {
            name: TextMode::Raw.as_str().to_string(),
            description: "Trim only; preserve dictated wording.".to_string(),
            deterministic: true,
            deterministic_mode: None,
            llm_instruction: None,
            style_profile: None,
        },
        ModeDefinition {
            name: TextMode::Clean.as_str().to_string(),
            description: "Remove safe fillers, normalize whitespace, and apply local rules."
                .to_string(),
            deterministic: true,
            deterministic_mode: None,
            llm_instruction: None,
            style_profile: None,
        },
        ModeDefinition {
            name: TextMode::Memo.as_str().to_string(),
            description: "Clean work notes without rewriting meaning.".to_string(),
            deterministic: true,
            deterministic_mode: None,
            llm_instruction: None,
            style_profile: None,
        },
        ModeDefinition {
            name: TextMode::CodingPrompt.as_str().to_string(),
            description:
                "Clean technical dictation and add sentence-ending punctuation when needed."
                    .to_string(),
            deterministic: true,
            deterministic_mode: None,
            llm_instruction: None,
            style_profile: None,
        },
        ModeDefinition {
            name: TextMode::EmailReply.as_str().to_string(),
            description: "Clean an email reply while keeping the user's wording.".to_string(),
            deterministic: true,
            deterministic_mode: None,
            llm_instruction: None,
            style_profile: None,
        },
        ModeDefinition {
            name: TextMode::SlackReply.as_str().to_string(),
            description: "Clean a concise chat reply while keeping the user's wording.".to_string(),
            deterministic: true,
            deterministic_mode: None,
            llm_instruction: None,
            style_profile: None,
        },
    ]
}

fn merge_custom_definition(definition: &mut ModeDefinition, custom: &ModeEntry) {
    if let Some(description) = &custom.description {
        definition.description = description.clone();
    }
    definition.deterministic = custom.llm_instruction.is_none();
    definition.deterministic_mode = custom.deterministic_mode.clone();
    definition.llm_instruction = custom.llm_instruction.clone();
    definition.style_profile = custom.style_profile.clone();
}

fn builtin_description(name: &str) -> Option<&'static str> {
    match name {
        "raw" => Some("Trim only; preserve dictated wording."),
        "clean" => Some("Remove safe fillers, normalize whitespace, and apply local rules."),
        "memo" => Some("Clean work notes without rewriting meaning."),
        "coding-prompt" => {
            Some("Clean technical dictation and add sentence-ending punctuation when needed.")
        }
        "email-reply" => Some("Clean an email reply while keeping the user's wording."),
        "slack-reply" => Some("Clean a concise chat reply while keeping the user's wording."),
        _ => None,
    }
}

fn apply_sentence_end_mode(raw_text: &str, rules: TextRules<'_>) -> String {
    ensure_sentence_end(&apply_deterministic_rules(raw_text, rules))
}

fn apply_slack_reply_mode(raw_text: &str, rules: TextRules<'_>) -> String {
    apply_deterministic_rules(raw_text, rules)
}

fn apply_deterministic_rules(raw_text: &str, rules: TextRules<'_>) -> String {
    let no_fillers = remove_safe_fillers(raw_text);
    let cleaned = clean_spacing(&no_fillers);
    let with_vocabulary = apply_vocabulary(&cleaned, rules.vocabulary);
    let with_snippets = apply_snippets(&with_vocabulary, rules.snippets);
    clean_spacing_preserving_newlines(&with_snippets)
}

fn remove_safe_fillers(raw_text: &str) -> String {
    raw_text
        .split_whitespace()
        .filter(|token| !is_safe_filler(token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_safe_filler(token: &str) -> bool {
    let trimmed = token.trim_matches(|ch: char| matches!(ch, ',' | '.' | '!' | '?' | ':' | ';'));
    matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "um" | "uh" | "umm" | "uhh"
    )
}

fn clean_spacing(raw_text: &str) -> String {
    let collapsed = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut cleaned = String::with_capacity(collapsed.len());

    for ch in collapsed.chars() {
        match ch {
            '.' | ',' | ';' | ':' | '!' | '?' => {
                while cleaned.ends_with(' ') {
                    cleaned.pop();
                }
                cleaned.push(ch);
            }
            ')' | ']' | '}' => {
                while cleaned.ends_with(' ') {
                    cleaned.pop();
                }
                cleaned.push(ch);
            }
            _ => cleaned.push(ch),
        }
    }

    cleaned.trim().to_string()
}

fn clean_spacing_preserving_newlines(raw_text: &str) -> String {
    raw_text
        .split('\n')
        .map(clean_spacing)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn apply_vocabulary(text: &str, entries: &[VocabularyEntry]) -> String {
    let replacements = entries
        .iter()
        .map(|entry| PhraseReplacement {
            trigger: entry.phrase.as_str(),
            replacement: entry.replacement.as_str(),
        })
        .collect::<Vec<_>>();
    apply_phrase_replacements(text, &replacements)
}

fn apply_snippets(text: &str, entries: &[SnippetEntry]) -> String {
    let replacements = entries
        .iter()
        .map(|entry| PhraseReplacement {
            trigger: entry.trigger.as_str(),
            replacement: entry.body.as_str(),
        })
        .collect::<Vec<_>>();
    apply_phrase_replacements(text, &replacements)
}

#[derive(Debug, Clone, Copy)]
struct PhraseReplacement<'a> {
    trigger: &'a str,
    replacement: &'a str,
}

#[derive(Debug, Clone)]
struct PreparedPhraseReplacement<'a> {
    trigger: &'a str,
    trigger_lower: String,
    replacement: &'a str,
}

fn apply_phrase_replacements(text: &str, replacements: &[PhraseReplacement<'_>]) -> String {
    if replacements.is_empty() || text.is_empty() {
        return text.to_string();
    }

    let mut ordered = replacements
        .iter()
        .filter(|replacement| !replacement.trigger.trim().is_empty())
        .map(|replacement| PreparedPhraseReplacement {
            trigger: replacement.trigger,
            trigger_lower: replacement.trigger.to_ascii_lowercase(),
            replacement: replacement.replacement,
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|a, b| {
        b.trigger
            .len()
            .cmp(&a.trigger.len())
            .then_with(|| a.trigger.cmp(b.trigger))
    });

    let lowered = text.to_ascii_lowercase();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;

    while index < text.len() {
        let mut matched = None;
        for replacement in &ordered {
            if phrase_matches(text, &lowered, index, &replacement.trigger_lower) {
                matched = Some(replacement);
                break;
            }
        }

        if let Some(replacement) = matched {
            output.push_str(replacement.replacement);
            index += replacement.trigger.len();
        } else {
            let ch = text[index..].chars().next().expect("valid char boundary");
            output.push(ch);
            index += ch.len_utf8();
        }
    }

    output
}

fn phrase_matches(text: &str, lowered: &str, index: usize, trigger_lower: &str) -> bool {
    let end = index + trigger_lower.len();
    if !text.is_char_boundary(index) || end > text.len() || !text.is_char_boundary(end) {
        return false;
    }

    if &lowered[index..end] != trigger_lower {
        return false;
    }

    let before = text[..index].chars().next_back();
    let after = text[end..].chars().next();
    is_phrase_boundary(before) && is_phrase_boundary(after)
}

fn is_phrase_boundary(ch: Option<char>) -> bool {
    ch.map(|ch| !ch.is_alphanumeric() && ch != '_' && ch != '-')
        .unwrap_or(true)
}

fn ensure_sentence_end(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.ends_with('.')
        || trimmed.ends_with('!')
        || trimmed.ends_with('?')
        || trimmed.ends_with('`')
    {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules<'a>(vocabulary: &'a [VocabularyEntry], snippets: &'a [SnippetEntry]) -> TextRules<'a> {
        TextRules {
            vocabulary,
            snippets,
        }
    }

    #[test]
    fn clean_mode_collapses_whitespace_removes_safe_fillers_and_fixes_punctuation() {
        assert_eq!(
            TextMode::Clean.process(
                "  um first line \n uh second line  .  ",
                TextRules::default()
            ),
            "first line second line."
        );
    }

    #[test]
    fn raw_mode_only_trims_edges() {
        assert_eq!(
            TextMode::Raw.process("  first   second  ", TextRules::default()),
            "first   second"
        );
    }

    #[test]
    fn vocabulary_replaces_phrases_case_insensitively() {
        let vocabulary = vec![VocabularyEntry {
            phrase: "super base".to_string(),
            replacement: "Supabase".to_string(),
        }];

        assert_eq!(
            TextMode::Clean.process("connect super base now", rules(&vocabulary, &[])),
            "connect Supabase now"
        );
    }

    #[test]
    fn snippets_use_longest_trigger_first() {
        let snippets = vec![
            SnippetEntry {
                trigger: "my".to_string(),
                body: "short".to_string(),
            },
            SnippetEntry {
                trigger: "my signature".to_string(),
                body: "Best,\nLuke".to_string(),
            },
        ];

        assert_eq!(
            TextMode::Clean.process("thanks my signature", rules(&[], &snippets)),
            "thanks Best,\nLuke"
        );
    }

    #[test]
    fn coding_prompt_preserves_code_like_tokens() {
        let raw = "uh keep fn(arg) arr[i] map {key: value} [text](url) src/main.rs cargo test --all camelCase snake_case https://example.com/api";

        assert_eq!(
            TextMode::CodingPrompt.process(raw, TextRules::default()),
            "keep fn(arg) arr[i] map {key: value} [text](url) src/main.rs cargo test --all camelCase snake_case https://example.com/api."
        );
    }

    #[test]
    fn phrase_matching_handles_non_ascii_char_boundaries() {
        let vocabulary = vec![VocabularyEntry {
            phrase: "cafe".to_string(),
            replacement: "Cafe".to_string(),
        }];

        assert_eq!(
            TextMode::Clean.process("café", rules(&vocabulary, &[])),
            "café"
        );
    }

    #[test]
    fn mode_registry_lists_all_builtin_modes() {
        let modes = mode_registry(&Default::default());
        assert_eq!(modes.len(), 6);
        assert!(modes.iter().any(|mode| mode.name == "coding-prompt"));
    }

    #[test]
    fn configured_mode_resolves_with_instruction_and_fallback() {
        let mut config = Config::default();
        config.modes.push(ModeEntry {
            name: "prompt".to_string(),
            description: Some("Coding prompt".to_string()),
            deterministic_mode: Some("clean".to_string()),
            llm_instruction: Some("Rewrite as a prompt".to_string()),
            style_profile: None,
        });

        let mode = resolve_mode(&config, "prompt").unwrap();

        assert_eq!(mode.name, "prompt");
        assert_eq!(mode.deterministic_mode, TextMode::Clean);
        assert_eq!(mode.llm_instruction.as_deref(), Some("Rewrite as a prompt"));
        assert_eq!(
            mode.process_deterministic("um ship it", TextRules::default()),
            "ship it"
        );
    }

    #[test]
    fn configured_mode_warns_for_unknown_deterministic_mode() {
        let mut config = Config::default();
        config.modes.push(ModeEntry {
            name: "prompt".to_string(),
            description: None,
            deterministic_mode: Some("typo".to_string()),
            llm_instruction: Some("Rewrite as a prompt".to_string()),
            style_profile: None,
        });

        let mode = resolve_mode(&config, "prompt").unwrap();

        assert_eq!(mode.deterministic_mode, TextMode::Memo);
        assert_eq!(mode.warnings.len(), 1);
        assert!(mode.warnings[0].contains("unknown deterministic mode"));
    }

    #[test]
    fn configured_mode_warns_for_missing_style_profile() {
        let mut config = Config::default();
        config.modes.push(ModeEntry {
            name: "prompt".to_string(),
            description: None,
            deterministic_mode: Some("clean".to_string()),
            llm_instruction: Some("Rewrite as a prompt".to_string()),
            style_profile: Some("missing".to_string()),
        });

        let mode = resolve_mode(&config, "prompt").unwrap();

        assert!(mode.style_profile.is_none());
        assert_eq!(mode.warnings.len(), 1);
        assert!(mode.warnings[0].contains("missing style profile"));
    }
}
