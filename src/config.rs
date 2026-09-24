use std::{
    env, fs,
    path::{Path, PathBuf},
};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::error::ComlinkError;

const RUNTIME_MODEL_NAMES: [&str; 2] = ["env", "cli"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionConfig {
    pub metadata: bool,
    pub transcripts: bool,
    pub audio: bool,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            metadata: true,
            transcripts: true,
            audio: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub name: String,
    pub path: PathBuf,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabularyEntry {
    pub phrase: String,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetEntry {
    pub trigger: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum LlmProvider {
    Ollama,
    #[serde(rename = "openai-compatible", alias = "open-ai-compatible")]
    OpenAiCompatible,
}

impl LlmProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenAiCompatible => "openai-compatible",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalLlmConfig {
    pub enabled: bool,
    pub provider: LlmProvider,
    pub endpoint: String,
    pub model: Option<String>,
    pub timeout_ms: u64,
}

impl Default for LocalLlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: LlmProvider::Ollama,
            endpoint: "http://127.0.0.1:11434/api/generate".to_string(),
            model: None,
            timeout_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeEntry {
    pub name: String,
    pub description: Option<String>,
    pub deterministic_mode: Option<String>,
    pub llm_instruction: Option<String>,
    pub style_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleExample {
    pub input: String,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleProfile {
    pub name: String,
    pub summary: String,
    pub examples: Vec<StyleExample>,
}

/// Local stdio MCP server (`comlink mcp`) settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    /// Allow MCP clients to start a meeting recording. Off by default: an
    /// agent must not be able to turn on the microphone until the user opts
    /// in with `comlink config set mcp.allow_start true`.
    #[serde(default)]
    pub allow_start: bool,
}

/// The only key `comlink config set` accepts.
pub const MCP_ALLOW_START_KEY: &str = "mcp.allow_start";
pub const MCP_ALLOW_START_ENV: &str = "COMLINK_MCP_ALLOW_START";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub history_enabled: bool,
    pub retention: RetentionConfig,
    pub selected_model: Option<String>,
    pub models: Vec<ModelEntry>,
    pub vocabulary: Vec<VocabularyEntry>,
    pub snippets: Vec<SnippetEntry>,
    pub modes: Vec<ModeEntry>,
    pub style_profiles: Vec<StyleProfile>,
    pub llm: LocalLlmConfig,
    #[serde(default)]
    pub mcp: McpConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            history_enabled: true,
            retention: RetentionConfig::default(),
            selected_model: None,
            models: Vec::new(),
            vocabulary: Vec::new(),
            snippets: Vec::new(),
            modes: Vec::new(),
            style_profiles: Vec::new(),
            llm: LocalLlmConfig::default(),
            mcp: McpConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigPaths {
    pub home_dir: PathBuf,
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub database_file: PathBuf,
    pub audio_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedConfig {
    pub paths: ConfigPaths,
    pub config: Config,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CliConfigOverrides {
    pub model: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
struct FileConfig {
    history_enabled: Option<bool>,
    retention: Option<FileRetentionConfig>,
    selected_model: Option<String>,
    models: Option<Vec<FileModelEntry>>,
    vocabulary: Option<Vec<VocabularyEntry>>,
    snippets: Option<Vec<SnippetEntry>>,
    modes: Option<Vec<ModeEntry>>,
    style_profiles: Option<Vec<StyleProfile>>,
    llm: Option<FileLocalLlmConfig>,
    mcp: Option<FileMcpConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct FileMcpConfig {
    allow_start: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct FileRetentionConfig {
    metadata: Option<bool>,
    transcripts: Option<bool>,
    audio: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct FileModelEntry {
    name: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct FileLocalLlmConfig {
    enabled: Option<bool>,
    provider: Option<LlmProvider>,
    endpoint: Option<String>,
    model: Option<String>,
    timeout_ms: Option<u64>,
}

pub fn load(overrides: CliConfigOverrides) -> Result<ResolvedConfig, ComlinkError> {
    let paths = resolve_paths()?;
    let mut config = Config::default();
    let mut sources = vec!["defaults".to_string()];

    merge_config_file(&mut config, &paths, &mut sources)?;

    merge_env(&mut config, &mut sources)?;
    merge_cli_overrides(&mut config, &mut sources, overrides);
    mark_selected_model(&mut config);

    Ok(ResolvedConfig {
        paths,
        config,
        sources,
    })
}

pub fn load_persistent() -> Result<ResolvedConfig, ComlinkError> {
    let paths = resolve_paths()?;
    let mut config = Config::default();
    let mut sources = vec!["defaults".to_string()];

    merge_config_file(&mut config, &paths, &mut sources)?;
    mark_selected_model(&mut config);

    Ok(ResolvedConfig {
        paths,
        config,
        sources,
    })
}

pub fn save(paths: &ConfigPaths, config: &Config) -> Result<(), ComlinkError> {
    if let Some(parent) = paths.config_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut to_write = config.clone();
    remove_runtime_models(&mut to_write);
    mark_selected_model(&mut to_write);
    let bytes = serde_json::to_vec_pretty(&to_write)?;
    // Atomic: a long-lived `comlink mcp` re-reads this file on every call and
    // must never observe a half-written config.
    crate::storage::write_atomic(&paths.config_file, &bytes)
}

/// Result of `comlink config set`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSetOutcome {
    pub key: &'static str,
    pub value: bool,
    pub config_file: PathBuf,
    /// Set when `COMLINK_MCP_ALLOW_START` is present and disagrees with the
    /// saved value, so the caller can warn that the env var wins.
    pub env_override: Option<String>,
}

/// Parse a `config set` boolean. Same spellings as the boolean env vars.
pub fn parse_bool_value(name: &'static str, value: &str) -> Result<bool, ComlinkError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(ComlinkError::InvalidConfigValue {
            name,
            value: other.to_string(),
        }),
    }
}

/// `comlink config set <key> <value>`. Only `mcp.allow_start` is settable.
/// Reads the persistent file config only (never env), changes that one key and
/// saves atomically, so unrelated keys survive and env values are never
/// written to disk.
pub fn set_key(key: &str, value: &str) -> Result<ConfigSetOutcome, ComlinkError> {
    if key != MCP_ALLOW_START_KEY {
        return Err(ComlinkError::UnknownConfigKey(key.to_string()));
    }
    let value = parse_bool_value(MCP_ALLOW_START_KEY, value)?;
    let mut resolved = load_persistent()?;
    resolved.config.mcp.allow_start = value;
    save(&resolved.paths, &resolved.config)?;
    let env_override = env::var(MCP_ALLOW_START_ENV)
        .ok()
        .filter(|raw| parse_bool_value(MCP_ALLOW_START_ENV, raw).ok() != Some(value));
    Ok(ConfigSetOutcome {
        key: MCP_ALLOW_START_KEY,
        value,
        config_file: resolved.paths.config_file,
        env_override,
    })
}

pub fn print(resolved: &ResolvedConfig, format: ConfigFormat) -> Result<(), ComlinkError> {
    match format {
        ConfigFormat::Json => {
            println!("{}", serde_json::to_string_pretty(resolved)?);
        }
        ConfigFormat::Text => {
            println!("home_dir: {}", resolved.paths.home_dir.display());
            println!("config_file: {}", resolved.paths.config_file.display());
            println!("database_file: {}", resolved.paths.database_file.display());
            println!("history_enabled: {}", resolved.config.history_enabled);
            println!(
                "retention: metadata={}, transcripts={}, audio={}",
                resolved.config.retention.metadata,
                resolved.config.retention.transcripts,
                resolved.config.retention.audio
            );
            println!(
                "selected_model: {}",
                resolved
                    .config
                    .selected_model
                    .as_deref()
                    .unwrap_or("<none>")
            );
            println!("mcp: allow_start={}", resolved.config.mcp.allow_start);
            println!("sources: {}", resolved.sources.join(", "));
        }
    }
    Ok(())
}

pub fn selected_model_path(config: &Config) -> Option<PathBuf> {
    let selected = config.selected_model.as_deref()?;
    config
        .models
        .iter()
        .find(|entry| entry.name == selected)
        .map(|entry| entry.path.clone())
}

pub fn select_model(resolved: &mut ResolvedConfig, name: &str, path: PathBuf) {
    if let Some(existing) = resolved
        .config
        .models
        .iter_mut()
        .find(|entry| entry.name == name)
    {
        existing.path = path;
    } else {
        resolved.config.models.push(ModelEntry {
            name: name.to_string(),
            path,
            selected: false,
        });
    }
    resolved.config.selected_model = Some(name.to_string());
    mark_selected_model(&mut resolved.config);
}

pub fn upsert_vocabulary(config: &mut Config, phrase: String, replacement: String) {
    if let Some(existing) = config
        .vocabulary
        .iter_mut()
        .find(|entry| entry.phrase.eq_ignore_ascii_case(&phrase))
    {
        existing.phrase = phrase;
        existing.replacement = replacement;
    } else {
        config.vocabulary.push(VocabularyEntry {
            phrase,
            replacement,
        });
    }
    sort_phrase_entries(&mut config.vocabulary, |entry| &entry.phrase);
}

pub fn remove_vocabulary(config: &mut Config, phrase: &str) -> bool {
    let original_len = config.vocabulary.len();
    config
        .vocabulary
        .retain(|entry| !entry.phrase.eq_ignore_ascii_case(phrase));
    config.vocabulary.len() != original_len
}

pub fn upsert_snippet(config: &mut Config, trigger: String, body: String) {
    if let Some(existing) = config
        .snippets
        .iter_mut()
        .find(|entry| entry.trigger.eq_ignore_ascii_case(&trigger))
    {
        existing.trigger = trigger;
        existing.body = body;
    } else {
        config.snippets.push(SnippetEntry { trigger, body });
    }
    sort_phrase_entries(&mut config.snippets, |entry| &entry.trigger);
}

pub fn remove_snippet(config: &mut Config, trigger: &str) -> bool {
    let original_len = config.snippets.len();
    config
        .snippets
        .retain(|entry| !entry.trigger.eq_ignore_ascii_case(trigger));
    config.snippets.len() != original_len
}

pub fn upsert_mode(
    config: &mut Config,
    name: String,
    description: Option<String>,
    deterministic_mode: Option<String>,
    llm_instruction: Option<String>,
    style_profile: Option<String>,
) {
    if let Some(existing) = config
        .modes
        .iter_mut()
        .find(|entry| entry.name.eq_ignore_ascii_case(&name))
    {
        existing.name = name;
        existing.description = description;
        existing.deterministic_mode = deterministic_mode;
        existing.llm_instruction = llm_instruction;
        existing.style_profile = style_profile;
    } else {
        config.modes.push(ModeEntry {
            name,
            description,
            deterministic_mode,
            llm_instruction,
            style_profile,
        });
    }
    sort_phrase_entries(&mut config.modes, |entry| &entry.name);
}

pub fn remove_mode(config: &mut Config, name: &str) -> bool {
    let original_len = config.modes.len();
    config
        .modes
        .retain(|entry| !entry.name.eq_ignore_ascii_case(name));
    config.modes.len() != original_len
}

pub fn upsert_style_profile(config: &mut Config, profile: StyleProfile) {
    if let Some(existing) = config
        .style_profiles
        .iter_mut()
        .find(|entry| entry.name.eq_ignore_ascii_case(&profile.name))
    {
        *existing = profile;
    } else {
        config.style_profiles.push(profile);
    }
    sort_phrase_entries(&mut config.style_profiles, |entry| &entry.name);
}

pub fn style_profile<'a>(config: &'a Config, name: &str) -> Option<&'a StyleProfile> {
    config
        .style_profiles
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(name))
}

fn resolve_paths() -> Result<ConfigPaths, ComlinkError> {
    let home_dir = if let Some(path) = env::var_os("COMLINK_HOME") {
        PathBuf::from(path)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(required_env("HOME")?)
            .join("Library")
            .join("Application Support")
            .join("comlink")
    } else if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(path).join("comlink")
    } else {
        PathBuf::from(required_env("HOME")?)
            .join(".config")
            .join("comlink")
    };

    let data_dir = if let Some(path) = env::var_os("COMLINK_DATA_DIR") {
        PathBuf::from(path)
    } else {
        home_dir.join("data")
    };

    Ok(ConfigPaths {
        config_file: home_dir.join("config.json"),
        database_file: data_dir.join("history.sqlite3"),
        audio_dir: data_dir.join("audio"),
        data_dir,
        home_dir,
    })
}

fn read_file_config(path: &Path) -> Result<FileConfig, ComlinkError> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|source| ComlinkError::ConfigParse {
        path: path.to_path_buf(),
        source,
    })
}

fn merge_config_file(
    config: &mut Config,
    paths: &ConfigPaths,
    sources: &mut Vec<String>,
) -> Result<(), ComlinkError> {
    if paths.config_file.exists() {
        let file_config = read_file_config(&paths.config_file)?;
        merge_file_config(config, file_config);
        sources.push(paths.config_file.display().to_string());
    }
    Ok(())
}

fn merge_file_config(config: &mut Config, file: FileConfig) {
    if let Some(value) = file.history_enabled {
        config.history_enabled = value;
    }
    if let Some(retention) = file.retention {
        if let Some(value) = retention.metadata {
            config.retention.metadata = value;
        }
        if let Some(value) = retention.transcripts {
            config.retention.transcripts = value;
        }
        if let Some(value) = retention.audio {
            config.retention.audio = value;
        }
    }
    if let Some(models) = file.models {
        config.models = models
            .into_iter()
            .map(|entry| ModelEntry {
                name: entry.name,
                path: entry.path,
                selected: false,
            })
            .collect();
    }
    if let Some(value) = file.selected_model {
        config.selected_model = Some(value);
    }
    if let Some(value) = file.vocabulary {
        config.vocabulary = value;
        sort_phrase_entries(&mut config.vocabulary, |entry| &entry.phrase);
    }
    if let Some(value) = file.snippets {
        config.snippets = value;
        sort_phrase_entries(&mut config.snippets, |entry| &entry.trigger);
    }
    if let Some(value) = file.modes {
        config.modes = value;
        sort_phrase_entries(&mut config.modes, |entry| &entry.name);
    }
    if let Some(value) = file.style_profiles {
        config.style_profiles = value;
        sort_phrase_entries(&mut config.style_profiles, |entry| &entry.name);
    }
    if let Some(llm) = file.llm {
        merge_file_llm_config(&mut config.llm, llm);
    }
    if let Some(value) = file.mcp.and_then(|mcp| mcp.allow_start) {
        config.mcp.allow_start = value;
    }
}

fn merge_env(config: &mut Config, sources: &mut Vec<String>) -> Result<(), ComlinkError> {
    if let Some(value) = bool_env("COMLINK_HISTORY_ENABLED")? {
        config.history_enabled = value;
        sources.push("COMLINK_HISTORY_ENABLED".to_string());
    }
    if let Some(value) = bool_env("COMLINK_RETAIN_METADATA")? {
        config.retention.metadata = value;
        sources.push("COMLINK_RETAIN_METADATA".to_string());
    }
    if let Some(value) = bool_env("COMLINK_RETAIN_TRANSCRIPTS")? {
        config.retention.transcripts = value;
        sources.push("COMLINK_RETAIN_TRANSCRIPTS".to_string());
    }
    if let Some(value) = bool_env("COMLINK_RETAIN_AUDIO")? {
        config.retention.audio = value;
        sources.push("COMLINK_RETAIN_AUDIO".to_string());
    }
    if let Some(value) = env::var_os("COMLINK_WHISPER_MODEL") {
        let path = PathBuf::from(value);
        upsert_env_model(config, path);
        sources.push("COMLINK_WHISPER_MODEL".to_string());
    }
    if let Some(value) = bool_env("COMLINK_LLM_ENABLED")? {
        config.llm.enabled = value;
        sources.push("COMLINK_LLM_ENABLED".to_string());
    }
    if let Ok(value) = env::var("COMLINK_LLM_PROVIDER") {
        config.llm.provider = parse_llm_provider(&value)?;
        sources.push("COMLINK_LLM_PROVIDER".to_string());
    }
    if let Ok(value) = env::var("COMLINK_LLM_ENDPOINT") {
        config.llm.endpoint = value;
        sources.push("COMLINK_LLM_ENDPOINT".to_string());
    }
    if let Ok(value) = env::var("COMLINK_LLM_MODEL") {
        config.llm.model = Some(value);
        sources.push("COMLINK_LLM_MODEL".to_string());
    }
    if let Some(value) = bool_env(MCP_ALLOW_START_ENV)? {
        config.mcp.allow_start = value;
        sources.push(MCP_ALLOW_START_ENV.to_string());
    }
    Ok(())
}

fn merge_file_llm_config(config: &mut LocalLlmConfig, file: FileLocalLlmConfig) {
    if let Some(value) = file.enabled {
        config.enabled = value;
    }
    if let Some(value) = file.provider {
        config.provider = value;
    }
    if let Some(value) = file.endpoint {
        config.endpoint = value;
    }
    if let Some(value) = file.model {
        config.model = Some(value);
    }
    if let Some(value) = file.timeout_ms {
        config.timeout_ms = value;
    }
}

fn parse_llm_provider(value: &str) -> Result<LlmProvider, ComlinkError> {
    match value {
        "ollama" => Ok(LlmProvider::Ollama),
        "openai-compatible" | "openai" => Ok(LlmProvider::OpenAiCompatible),
        _ => Err(ComlinkError::InvalidConfigValue {
            name: "COMLINK_LLM_PROVIDER",
            value: value.to_string(),
        }),
    }
}

fn merge_cli_overrides(
    config: &mut Config,
    sources: &mut Vec<String>,
    overrides: CliConfigOverrides,
) {
    if let Some(path) = overrides.model {
        upsert_named_model(config, "cli", path);
        config.selected_model = Some("cli".to_string());
        sources.push("--model".to_string());
    }
}

fn upsert_env_model(config: &mut Config, path: PathBuf) {
    upsert_named_model(config, "env", path);
    config.selected_model = Some("env".to_string());
}

fn upsert_named_model(config: &mut Config, name: &str, path: PathBuf) {
    if let Some(existing) = config.models.iter_mut().find(|entry| entry.name == name) {
        existing.path = path;
    } else {
        config.models.push(ModelEntry {
            name: name.to_string(),
            path,
            selected: false,
        });
    }
}

fn mark_selected_model(config: &mut Config) {
    let selected = config.selected_model.clone();
    for entry in &mut config.models {
        entry.selected = selected.as_ref() == Some(&entry.name);
    }
}

fn remove_runtime_models(config: &mut Config) {
    config
        .models
        .retain(|entry| !RUNTIME_MODEL_NAMES.contains(&entry.name.as_str()));
    if config
        .selected_model
        .as_deref()
        .is_some_and(|name| RUNTIME_MODEL_NAMES.contains(&name))
    {
        config.selected_model = None;
    }
}

fn sort_phrase_entries<T>(entries: &mut [T], phrase: impl Fn(&T) -> &str) {
    entries.sort_by(|a, b| {
        phrase(b).len().cmp(&phrase(a).len()).then_with(|| {
            phrase(a)
                .to_ascii_lowercase()
                .cmp(&phrase(b).to_ascii_lowercase())
        })
    });
}

fn bool_env(name: &'static str) -> Result<Option<bool>, ComlinkError> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    let text = value.to_string_lossy().to_ascii_lowercase();
    match text.as_str() {
        "1" | "true" | "yes" | "on" => Ok(Some(true)),
        "0" | "false" | "no" | "off" => Ok(Some(false)),
        _ => Err(ComlinkError::InvalidConfigValue { name, value: text }),
    }
}

fn required_env(name: &'static str) -> Result<String, ComlinkError> {
    env::var(name).map_err(|_| ComlinkError::ConfigHomeMissing(name))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn config_env_overrides_file_and_defaults() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join("config.json");
        fs::write(
            &config_file,
            r#"{
              "history_enabled": false,
              "retention": {"transcripts": true},
              "selected_model": "file",
              "models": [{"name": "file", "path": "/tmp/file-model.bin"}],
              "vocabulary": [{"phrase": "super base", "replacement": "Supabase"}],
              "snippets": [{"trigger": "my signature", "body": "Best,\nLuke"}]
            }"#,
        )
        .unwrap();

        let previous_home = env::var_os("COMLINK_HOME");
        let previous_history = env::var_os("COMLINK_HISTORY_ENABLED");
        let previous_transcripts = env::var_os("COMLINK_RETAIN_TRANSCRIPTS");
        let previous_model = env::var_os("COMLINK_WHISPER_MODEL");
        env::set_var("COMLINK_HOME", dir.path());
        env::set_var("COMLINK_HISTORY_ENABLED", "true");
        env::set_var("COMLINK_RETAIN_TRANSCRIPTS", "false");
        env::set_var("COMLINK_WHISPER_MODEL", "/tmp/env-model.bin");

        let resolved = load(CliConfigOverrides::default()).unwrap();

        restore_env("COMLINK_HOME", previous_home);
        restore_env("COMLINK_HISTORY_ENABLED", previous_history);
        restore_env("COMLINK_RETAIN_TRANSCRIPTS", previous_transcripts);
        restore_env("COMLINK_WHISPER_MODEL", previous_model);

        assert!(resolved.config.history_enabled);
        assert!(!resolved.config.retention.transcripts);
        assert_eq!(resolved.config.selected_model.as_deref(), Some("env"));
        assert_eq!(
            selected_model_path(&resolved.config).unwrap(),
            PathBuf::from("/tmp/env-model.bin")
        );
        assert!(resolved
            .sources
            .contains(&"COMLINK_WHISPER_MODEL".to_string()));
        assert_eq!(resolved.config.vocabulary[0].replacement, "Supabase");
        assert_eq!(resolved.config.snippets[0].body, "Best,\nLuke");
    }

    #[test]
    fn invalid_bool_env_is_rejected() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let previous_home = env::var_os("COMLINK_HOME");
        let previous = env::var_os("COMLINK_RETAIN_AUDIO");
        env::set_var("COMLINK_HOME", dir.path());
        env::set_var("COMLINK_RETAIN_AUDIO", "maybe");

        let error = load(CliConfigOverrides::default()).unwrap_err();

        restore_env("COMLINK_HOME", previous_home);
        restore_env("COMLINK_RETAIN_AUDIO", previous);

        assert!(error.to_string().contains("COMLINK_RETAIN_AUDIO"));
    }

    #[test]
    fn llm_provider_uses_advertised_openai_compatible_spelling() {
        let serialized = serde_json::to_string(&LlmProvider::OpenAiCompatible).unwrap();
        assert_eq!(serialized, r#""openai-compatible""#);

        let provider: LlmProvider = serde_json::from_str(r#""openai-compatible""#).unwrap();
        assert_eq!(provider, LlmProvider::OpenAiCompatible);
    }

    #[test]
    fn llm_provider_accepts_legacy_kebab_case_spelling() {
        let provider: LlmProvider = serde_json::from_str(r#""open-ai-compatible""#).unwrap();

        assert_eq!(provider, LlmProvider::OpenAiCompatible);
    }

    #[test]
    fn save_omits_runtime_models() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            home_dir: dir.path().to_path_buf(),
            config_file: dir.path().join("config.json"),
            data_dir: dir.path().join("data"),
            database_file: dir.path().join("data/history.sqlite3"),
            audio_dir: dir.path().join("data/audio"),
        };
        let config = Config {
            selected_model: Some("file".to_string()),
            models: vec![
                ModelEntry {
                    name: "file".to_string(),
                    path: PathBuf::from("/tmp/file-model.bin"),
                    selected: false,
                },
                ModelEntry {
                    name: "env".to_string(),
                    path: PathBuf::from("/tmp/env-model.bin"),
                    selected: true,
                },
                ModelEntry {
                    name: "cli".to_string(),
                    path: PathBuf::from("/tmp/cli-model.bin"),
                    selected: false,
                },
            ],
            vocabulary: vec![VocabularyEntry {
                phrase: "super base".to_string(),
                replacement: "Supabase".to_string(),
            }],
            snippets: vec![SnippetEntry {
                trigger: "my signature".to_string(),
                body: "Best,\nLuke".to_string(),
            }],
            ..Config::default()
        };

        save(&paths, &config).unwrap();

        let text = fs::read_to_string(&paths.config_file).unwrap();
        assert!(text.contains("\"file\""));
        assert!(text.contains("\"super base\""));
        assert!(text.contains("\"my signature\""));
        assert!(!text.contains("\"env\""));
        assert!(!text.contains("\"cli\""));
    }

    #[test]
    fn load_persistent_ignores_runtime_env_overrides() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join("config.json");
        fs::write(
            &config_file,
            r#"{
              "selected_model": "file",
              "models": [{"name": "file", "path": "/tmp/file-model.bin"}]
            }"#,
        )
        .unwrap();

        let previous_home = env::var_os("COMLINK_HOME");
        let previous_model = env::var_os("COMLINK_WHISPER_MODEL");
        let previous_llm_enabled = env::var_os("COMLINK_LLM_ENABLED");
        let previous_llm_endpoint = env::var_os("COMLINK_LLM_ENDPOINT");
        let previous_llm_model = env::var_os("COMLINK_LLM_MODEL");
        env::set_var("COMLINK_HOME", dir.path());
        env::set_var("COMLINK_WHISPER_MODEL", "/tmp/env-model.bin");
        env::set_var("COMLINK_LLM_ENABLED", "true");
        env::set_var("COMLINK_LLM_ENDPOINT", "http://127.0.0.1:9999/api/generate");
        env::set_var("COMLINK_LLM_MODEL", "env-llm");

        let resolved = load_persistent().unwrap();

        restore_env("COMLINK_HOME", previous_home);
        restore_env("COMLINK_WHISPER_MODEL", previous_model);
        restore_env("COMLINK_LLM_ENABLED", previous_llm_enabled);
        restore_env("COMLINK_LLM_ENDPOINT", previous_llm_endpoint);
        restore_env("COMLINK_LLM_MODEL", previous_llm_model);

        assert_eq!(resolved.config.selected_model.as_deref(), Some("file"));
        assert_eq!(resolved.config.models.len(), 1);
        assert_eq!(
            selected_model_path(&resolved.config).unwrap(),
            PathBuf::from("/tmp/file-model.bin")
        );
        assert!(!resolved
            .sources
            .contains(&"COMLINK_WHISPER_MODEL".to_string()));
        assert!(!resolved.config.llm.enabled);
        assert_eq!(
            resolved.config.llm.endpoint,
            "http://127.0.0.1:11434/api/generate"
        );
        assert_eq!(resolved.config.llm.model, None);
        assert!(!resolved
            .sources
            .contains(&"COMLINK_LLM_ENABLED".to_string()));
    }

    #[test]
    fn vocab_and_snippets_are_upserted_and_removed_case_insensitively() {
        let mut config = Config::default();

        upsert_vocabulary(
            &mut config,
            "super base".to_string(),
            "Supabase".to_string(),
        );
        upsert_vocabulary(
            &mut config,
            "Super Base".to_string(),
            "SUPABASE".to_string(),
        );
        upsert_snippet(&mut config, "my signature".to_string(), "Best".to_string());
        upsert_snippet(
            &mut config,
            "My Signature".to_string(),
            "Regards".to_string(),
        );

        assert_eq!(config.vocabulary.len(), 1);
        assert_eq!(config.vocabulary[0].replacement, "SUPABASE");
        assert_eq!(config.snippets.len(), 1);
        assert_eq!(config.snippets[0].body, "Regards");
        assert!(remove_vocabulary(&mut config, "SUPER BASE"));
        assert!(remove_snippet(&mut config, "MY SIGNATURE"));
        assert!(config.vocabulary.is_empty());
        assert!(config.snippets.is_empty());
    }

    #[test]
    fn selected_model_path_is_none_when_registry_entry_is_missing() {
        let config = Config {
            selected_model: Some("missing".to_string()),
            models: Vec::new(),
            ..Config::default()
        };

        assert_eq!(selected_model_path(&config), None);
    }

    /// Run `body` with `COMLINK_HOME` pointed at a fresh dir and the MCP env
    /// var set to `mcp_env` (or unset), restoring both afterwards.
    fn with_isolated_home<T>(mcp_env: Option<&str>, body: impl FnOnce(&Path) -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let previous_home = env::var_os("COMLINK_HOME");
        let previous_mcp = env::var_os(MCP_ALLOW_START_ENV);
        env::set_var("COMLINK_HOME", dir.path());
        match mcp_env {
            Some(value) => env::set_var(MCP_ALLOW_START_ENV, value),
            None => env::remove_var(MCP_ALLOW_START_ENV),
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(dir.path())));
        restore_env("COMLINK_HOME", previous_home);
        restore_env(MCP_ALLOW_START_ENV, previous_mcp);
        match result {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    #[test]
    fn mcp_allow_start_defaults_false_then_file_then_env_wins() {
        with_isolated_home(None, |home| {
            assert!(
                !load(CliConfigOverrides::default())
                    .unwrap()
                    .config
                    .mcp
                    .allow_start
            );
            fs::write(home.join("config.json"), r#"{"mcp":{"allow_start":true}}"#).unwrap();
            assert!(
                load(CliConfigOverrides::default())
                    .unwrap()
                    .config
                    .mcp
                    .allow_start
            );
        });
        with_isolated_home(Some("false"), |home| {
            fs::write(home.join("config.json"), r#"{"mcp":{"allow_start":true}}"#).unwrap();
            let resolved = load(CliConfigOverrides::default()).unwrap();
            assert!(!resolved.config.mcp.allow_start, "env overrides file");
            assert!(resolved.sources.contains(&MCP_ALLOW_START_ENV.to_string()));
            // The persistent view ignores env.
            assert!(load_persistent().unwrap().config.mcp.allow_start);
        });
        with_isolated_home(Some("1"), |_| {
            assert!(
                load(CliConfigOverrides::default())
                    .unwrap()
                    .config
                    .mcp
                    .allow_start
            );
        });
    }

    #[test]
    fn mcp_allow_start_env_rejects_non_boolean() {
        with_isolated_home(Some("sometimes"), |_| {
            let error = load(CliConfigOverrides::default()).unwrap_err();
            assert!(matches!(
                error,
                ComlinkError::InvalidConfigValue { name, .. } if name == MCP_ALLOW_START_ENV
            ));
        });
    }

    #[test]
    fn config_set_changes_only_allow_start_and_never_persists_env() {
        with_isolated_home(Some("true"), |home| {
            let file = home.join("config.json");
            fs::write(
                &file,
                r#"{
                  "history_enabled": false,
                  "selected_model": "base",
                  "models": [{"name": "base", "path": "/tmp/base.bin"}],
                  "vocabulary": [{"phrase": "super base", "replacement": "Supabase"}],
                  "modes": [{"name": "standup", "deterministic_mode": "memo"}]
                }"#,
            )
            .unwrap();
            env::set_var("COMLINK_RETAIN_AUDIO", "true");

            let outcome = set_key("mcp.allow_start", "false").unwrap();
            env::remove_var("COMLINK_RETAIN_AUDIO");

            assert_eq!(outcome.key, "mcp.allow_start");
            assert!(!outcome.value);
            assert_eq!(outcome.config_file, file);
            assert_eq!(outcome.env_override.as_deref(), Some("true"));

            let saved: serde_json::Value =
                serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
            assert_eq!(saved["mcp"]["allow_start"], false);
            assert_eq!(saved["history_enabled"], false);
            assert_eq!(saved["selected_model"], "base");
            assert_eq!(saved["models"][0]["path"], "/tmp/base.bin");
            assert_eq!(saved["vocabulary"][0]["replacement"], "Supabase");
            assert_eq!(saved["modes"][0]["name"], "standup");
            // Env-derived values never reach disk.
            assert_eq!(saved["retention"]["audio"], false);

            let outcome = set_key("mcp.allow_start", "true").unwrap();
            assert!(outcome.value);
            assert_eq!(outcome.env_override, None, "env agrees with the new value");
            assert!(load_persistent().unwrap().config.mcp.allow_start);
        });
    }

    #[test]
    fn config_set_rejects_unknown_keys_and_bad_booleans_without_writing() {
        with_isolated_home(None, |home| {
            let error = set_key("retention.audio", "true").unwrap_err();
            assert!(
                matches!(error, ComlinkError::UnknownConfigKey(ref key) if key == "retention.audio")
            );
            let error = set_key("mcp.allow_start", "maybe").unwrap_err();
            assert!(matches!(error, ComlinkError::InvalidConfigValue { .. }));
            assert!(!home.join("config.json").exists());
        });
    }

    #[test]
    fn concurrent_reloads_never_see_a_partial_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            home_dir: dir.path().to_path_buf(),
            config_file: dir.path().join("config.json"),
            data_dir: dir.path().join("data"),
            database_file: dir.path().join("data/history.sqlite3"),
            audio_dir: dir.path().join("data/audio"),
        };
        let mut config = Config::default();
        // Big enough that a non-atomic write is observable mid-way.
        for index in 0..400 {
            config.vocabulary.push(VocabularyEntry {
                phrase: format!("phrase number {index}"),
                replacement: format!("Replacement {index}"),
            });
        }
        save(&paths, &config).unwrap();

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let stop = stop.clone();
            let file = paths.config_file.clone();
            std::thread::spawn(move || {
                let mut reads = 0usize;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    read_file_config(&file).expect("every read parses");
                    reads += 1;
                }
                reads
            })
        };
        for round in 0..150 {
            config.mcp.allow_start = round % 2 == 0;
            save(&paths, &config).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(reader.join().unwrap() > 0);
        let leftovers = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "only config.json remains (no temp files)");
    }

    fn restore_env(name: &str, previous: Option<std::ffi::OsString>) {
        match previous {
            Some(value) => env::set_var(name, value),
            None => env::remove_var(name),
        }
    }
}
