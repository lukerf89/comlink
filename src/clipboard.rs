use std::{
    env,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::error::ComlinkError;

const CLIPBOARD_COMMAND_PATH: &str = "/usr/bin:/bin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardCommandState {
    Found(PathBuf),
    Missing(PathBuf),
    NotExecutable(PathBuf),
}

impl ClipboardCommandState {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Found(_))
    }

    pub fn path(&self) -> &PathBuf {
        match self {
            Self::Found(path) | Self::Missing(path) | Self::NotExecutable(path) => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardReport {
    pub copy: ClipboardCommandState,
    pub read: ClipboardCommandState,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CopyOptions {
    pub restore_previous: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyResult {
    pub restored_previous: bool,
}

pub fn copy_text(text: &str) -> Result<(), ComlinkError> {
    copy_text_with_options(text, CopyOptions::default()).map(|_| ())
}

pub fn copy_text_with_options(
    text: &str,
    options: CopyOptions,
) -> Result<CopyResult, ComlinkError> {
    let previous = if options.restore_previous {
        Some(read_text().map_err(|error| {
            ComlinkError::ClipboardFailed(format!(
                "cannot restore clipboard because current clipboard could not be read: {error}"
            ))
        })?)
    } else {
        None
    };

    write_text(text)?;

    if let Some(previous) = previous {
        write_text(&previous).map_err(|error| {
            ComlinkError::ClipboardFailed(format!(
                "copied final text but failed to restore previous clipboard: {error}"
            ))
        })?;
        Ok(CopyResult {
            restored_previous: true,
        })
    } else {
        Ok(CopyResult {
            restored_previous: false,
        })
    }
}

pub fn inspect() -> ClipboardReport {
    ClipboardReport {
        copy: inspect_command("COMLINK_PBCOPY", "pbcopy"),
        read: inspect_command("COMLINK_PBPASTE", "pbpaste"),
    }
}

fn write_text(text: &str) -> Result<(), ComlinkError> {
    let command = copy_command();

    let mut child = clipboard_command(&command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ComlinkError::ClipboardFailed(format!("failed to start {}: {error}", command.display()))
        })?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ComlinkError::ClipboardFailed("clipboard stdin unavailable".to_string()))?;
    stdin
        .write_all(text.as_bytes())
        .map_err(|error| ComlinkError::ClipboardFailed(error.to_string()))?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .map_err(|error| ComlinkError::ClipboardFailed(error.to_string()))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(ComlinkError::ClipboardFailed(if stderr.is_empty() {
            "clipboard command exited unsuccessfully".to_string()
        } else {
            stderr
        }))
    }
}

fn read_text() -> Result<String, ComlinkError> {
    let command = read_command();
    let output = clipboard_command(&command)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            ComlinkError::ClipboardFailed(format!("failed to start {}: {error}", command.display()))
        })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(ComlinkError::ClipboardFailed(if stderr.is_empty() {
            format!("{} exited unsuccessfully", command.display())
        } else {
            stderr
        }))
    }
}

fn clipboard_command(command: &Path) -> Command {
    let mut child = Command::new(command);
    child.env("PATH", CLIPBOARD_COMMAND_PATH);
    child
}

fn copy_command() -> PathBuf {
    env::var_os("COMLINK_PBCOPY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("pbcopy"))
}

fn read_command() -> PathBuf {
    env::var_os("COMLINK_PBPASTE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("pbpaste"))
}

fn inspect_command(env_name: &str, default_command: &str) -> ClipboardCommandState {
    let command = env::var_os(env_name)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default_command));
    if command.components().count() == 1 {
        if let Ok(path) = which::which(&command) {
            return ClipboardCommandState::Found(path);
        }
        return ClipboardCommandState::Missing(command);
    }
    if is_executable(&command) {
        ClipboardCommandState::Found(command)
    } else if command.exists() {
        ClipboardCommandState::NotExecutable(command)
    } else {
        ClipboardCommandState::Missing(command)
    }
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use std::{env, fs, sync::Mutex};

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn inspect_reports_missing_explicit_copy_command() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = env::var_os("COMLINK_PBCOPY");
        let missing = PathBuf::from("/definitely/missing/comlink-pbcopy");
        env::set_var("COMLINK_PBCOPY", &missing);

        let report = inspect();

        restore_env("COMLINK_PBCOPY", previous);
        assert_eq!(report.copy, ClipboardCommandState::Missing(missing));
    }

    #[test]
    #[cfg(unix)]
    fn copy_can_restore_previous_clipboard_when_requested() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("clipboard.txt");
        fs::write(&store, "previous").unwrap();
        let pbcopy = dir.path().join("pbcopy");
        let pbpaste = dir.path().join("pbpaste");
        fs::write(&pbcopy, format!("#!/bin/sh\ncat > '{}'\n", store.display())).unwrap();
        fs::write(&pbpaste, format!("#!/bin/sh\ncat '{}'\n", store.display())).unwrap();
        make_executable(&pbcopy);
        make_executable(&pbpaste);

        let previous_copy = env::var_os("COMLINK_PBCOPY");
        let previous_paste = env::var_os("COMLINK_PBPASTE");
        env::set_var("COMLINK_PBCOPY", &pbcopy);
        env::set_var("COMLINK_PBPASTE", &pbpaste);

        let result = copy_text_with_options(
            "new text",
            CopyOptions {
                restore_previous: true,
            },
        )
        .unwrap();

        restore_env("COMLINK_PBCOPY", previous_copy);
        restore_env("COMLINK_PBPASTE", previous_paste);
        assert!(result.restored_previous);
        assert_eq!(fs::read_to_string(store).unwrap(), "previous");
    }

    #[cfg(unix)]
    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn restore_env(name: &str, previous: Option<std::ffi::OsString>) {
        match previous {
            Some(value) => env::set_var(name, value),
            None => env::remove_var(name),
        }
    }
}
