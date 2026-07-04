use std::{
    env,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::error::ComlinkError;

pub fn copy_text(text: &str) -> Result<(), ComlinkError> {
    let command = env::var_os("COMLINK_PBCOPY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("pbcopy"));

    let mut child = Command::new(&command)
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
