use std::{
    env,
    path::{Path, PathBuf},
};

use crate::error::ComlinkError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyState {
    Found(PathBuf),
    Missing,
    NotExecutable(PathBuf),
}

impl DependencyState {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Found(_))
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Found(path) | Self::NotExecutable(path) => Some(path),
            Self::Missing => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DependencyReport {
    pub ffmpeg: DependencyState,
    pub ffprobe: DependencyState,
    pub whisper_cpp: DependencyState,
    pub whisper_model: DependencyState,
}

#[derive(Debug, Clone)]
pub struct RuntimeDeps {
    pub ffmpeg: PathBuf,
    pub ffprobe: Option<PathBuf>,
    pub whisper_cpp: PathBuf,
    pub whisper_model: PathBuf,
}

pub fn inspect() -> DependencyReport {
    DependencyReport {
        ffmpeg: resolve_binary("COMLINK_FFMPEG", &["ffmpeg"]),
        ffprobe: resolve_binary("COMLINK_FFPROBE", &["ffprobe"]),
        whisper_cpp: resolve_binary(
            "COMLINK_WHISPER_CPP",
            &["whisper-cli", "whisper.cpp", "whisper", "main"],
        ),
        whisper_model: resolve_model("COMLINK_WHISPER_MODEL"),
    }
}

pub fn runtime_from_env(model_override: Option<PathBuf>) -> Result<RuntimeDeps, ComlinkError> {
    let ffmpeg = require_binary("COMLINK_FFMPEG", &["ffmpeg"], "ffmpeg")?;
    let ffprobe = match resolve_binary("COMLINK_FFPROBE", &["ffprobe"]) {
        DependencyState::Found(path) => Some(path),
        _ => None,
    };
    let whisper_cpp = require_binary(
        "COMLINK_WHISPER_CPP",
        &["whisper-cli", "whisper.cpp", "whisper", "main"],
        "whisper.cpp",
    )?;
    let whisper_model = match model_override {
        Some(path) => require_existing_model(path)?,
        None => env::var_os("COMLINK_WHISPER_MODEL")
            .map(PathBuf::from)
            .ok_or(ComlinkError::ModelMissing)
            .and_then(require_existing_model)?,
    };

    Ok(RuntimeDeps {
        ffmpeg,
        ffprobe,
        whisper_cpp,
        whisper_model,
    })
}

fn require_binary(
    env_name: &'static str,
    candidates: &[&str],
    label: &'static str,
) -> Result<PathBuf, ComlinkError> {
    match resolve_binary(env_name, candidates) {
        DependencyState::Found(path) => Ok(path),
        DependencyState::NotExecutable(path) => Err(ComlinkError::DependencyNotExecutable {
            name: env_name,
            path,
        }),
        DependencyState::Missing => Err(ComlinkError::DependencyMissing(label)),
    }
}

fn resolve_binary(env_name: &str, candidates: &[&str]) -> DependencyState {
    if let Some(value) = env::var_os(env_name) {
        let path = PathBuf::from(value);
        return if is_executable(&path) {
            DependencyState::Found(path)
        } else {
            DependencyState::NotExecutable(path)
        };
    }

    for candidate in candidates {
        if let Ok(path) = which::which(candidate) {
            return DependencyState::Found(path);
        }
    }

    DependencyState::Missing
}

fn resolve_model(env_name: &str) -> DependencyState {
    match env::var_os(env_name).map(PathBuf::from) {
        Some(path) if path.is_file() => DependencyState::Found(path),
        Some(path) => DependencyState::NotExecutable(path),
        None => DependencyState::Missing,
    }
}

fn require_existing_model(path: PathBuf) -> Result<PathBuf, ComlinkError> {
    if path.is_file() {
        Ok(path)
    } else {
        Err(ComlinkError::ModelPathMissing(path))
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
