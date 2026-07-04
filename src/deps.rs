use std::{
    env,
    path::{Path, PathBuf},
};

use crate::error::ComlinkError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyState {
    Found(PathBuf),
    Missing,
    NotFound(PathBuf),
    NotExecutable(PathBuf),
}

impl DependencyState {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Found(_))
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Found(path) | Self::NotFound(path) | Self::NotExecutable(path) => Some(path),
            Self::Missing => None,
        }
    }
}

const WHISPER_CPP_CANDIDATES: &[&str] = &["whisper-cli", "whisper.cpp"];

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
        whisper_cpp: resolve_binary("COMLINK_WHISPER_CPP", WHISPER_CPP_CANDIDATES),
        whisper_model: resolve_model("COMLINK_WHISPER_MODEL"),
    }
}

pub fn runtime_from_env(model_override: Option<PathBuf>) -> Result<RuntimeDeps, ComlinkError> {
    let ffmpeg = require_binary("COMLINK_FFMPEG", &["ffmpeg"], "ffmpeg")?;
    let ffprobe = match resolve_binary("COMLINK_FFPROBE", &["ffprobe"]) {
        DependencyState::Found(path) => Some(path),
        _ => None,
    };
    let whisper_cpp = require_binary("COMLINK_WHISPER_CPP", WHISPER_CPP_CANDIDATES, "whisper.cpp")?;
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
        DependencyState::NotFound(path) => Err(ComlinkError::DependencyPathMissing {
            name: env_name,
            path,
        }),
        DependencyState::NotExecutable(path) => Err(ComlinkError::DependencyNotExecutable {
            name: env_name,
            path,
        }),
        DependencyState::Missing => Err(ComlinkError::DependencyMissing(label)),
    }
}

fn resolve_binary(env_name: &str, candidates: &[&str]) -> DependencyState {
    if let Some(value) = env::var_os(env_name) {
        let path = PathBuf::from(&value);
        if is_executable(&path) {
            return DependencyState::Found(path);
        }
        if path.exists() {
            return DependencyState::NotExecutable(path);
        }
        if !is_path_like(&path) {
            if let Ok(resolved) = which::which(&value) {
                return DependencyState::Found(resolved);
            }
        }
        return DependencyState::NotFound(path);
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
        Some(path) => DependencyState::NotFound(path),
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

fn is_path_like(path: &Path) -> bool {
    path.is_absolute() || path.components().count() > 1
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Mutex};

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn explicit_env_override_accepts_command_name_on_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let command = dir.path().join("mock-whisper");
        make_executable(&command);

        let previous_path = env::var_os("PATH");
        let previous_override = env::var_os("COMLINK_WHISPER_CPP");
        env::set_var("PATH", dir.path());
        env::set_var("COMLINK_WHISPER_CPP", "mock-whisper");

        let state = resolve_binary("COMLINK_WHISPER_CPP", WHISPER_CPP_CANDIDATES);

        restore_env("PATH", previous_path);
        restore_env("COMLINK_WHISPER_CPP", previous_override);

        assert_eq!(state, DependencyState::Found(command));
    }

    #[test]
    #[cfg(unix)]
    fn auto_detect_ignores_generic_main_binary() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        make_executable(&dir.path().join("main"));

        let previous_path = env::var_os("PATH");
        let previous_override = env::var_os("COMLINK_WHISPER_CPP");
        env::set_var("PATH", dir.path());
        env::remove_var("COMLINK_WHISPER_CPP");

        let state = resolve_binary("COMLINK_WHISPER_CPP", WHISPER_CPP_CANDIDATES);

        restore_env("PATH", previous_path);
        restore_env("COMLINK_WHISPER_CPP", previous_override);

        assert_eq!(state, DependencyState::Missing);
    }

    #[test]
    fn missing_override_reports_not_found() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_override = env::var_os("COMLINK_WHISPER_CPP");
        let missing = PathBuf::from("/definitely/not/a/comlink/binary");
        env::set_var("COMLINK_WHISPER_CPP", &missing);

        let state = resolve_binary("COMLINK_WHISPER_CPP", WHISPER_CPP_CANDIDATES);

        restore_env("COMLINK_WHISPER_CPP", previous_override);

        assert_eq!(state, DependencyState::NotFound(missing));
    }

    fn restore_env(name: &str, previous: Option<std::ffi::OsString>) {
        match previous {
            Some(value) => env::set_var(name, value),
            None => env::remove_var(name),
        }
    }
}
