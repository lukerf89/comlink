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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub history_enabled: bool,
    pub retention: RetentionConfig,
    pub selected_model: Option<String>,
    pub models: Vec<ModelEntry>,
    pub vocabulary: Vec<VocabularyEntry>,
    pub snippets: Vec<SnippetEntry>,
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
    fs::write(&paths.config_file, bytes)?;
    Ok(())
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
    Ok(())
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
    fn load_persistent_ignores_runtime_env_model() {
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
        env::set_var("COMLINK_HOME", dir.path());
        env::set_var("COMLINK_WHISPER_MODEL", "/tmp/env-model.bin");

        let resolved = load_persistent().unwrap();

        restore_env("COMLINK_HOME", previous_home);
        restore_env("COMLINK_WHISPER_MODEL", previous_model);

        assert_eq!(resolved.config.selected_model.as_deref(), Some("file"));
        assert_eq!(resolved.config.models.len(), 1);
        assert_eq!(
            selected_model_path(&resolved.config).unwrap(),
            PathBuf::from("/tmp/file-model.bin")
        );
        assert!(!resolved
            .sources
            .contains(&"COMLINK_WHISPER_MODEL".to_string()));
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

    fn restore_env(name: &str, previous: Option<std::ffi::OsString>) {
        match previous {
            Some(value) => env::set_var(name, value),
            None => env::remove_var(name),
        }
    }
}
