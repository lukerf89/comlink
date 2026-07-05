use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::{
    asr::SourceMetadata,
    config::{ConfigPaths, RetentionConfig},
    error::ComlinkError,
    output::TranscriptOutput,
};

#[derive(Debug, Clone, Serialize)]
pub struct StoredSession {
    pub id: String,
    pub schema_version: String,
    pub created_at_ms: i64,
    pub mode: String,
    pub copied: bool,
    pub engine: String,
    pub model: String,
    pub duration_ms: u64,
    pub source: SourceMetadata,
    pub raw_text: Option<String>,
    pub final_text: Option<String>,
    pub audio_path: Option<String>,
    pub segments: Vec<StoredSegment>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredSessionSummary {
    pub id: String,
    pub created_at_ms: i64,
    pub mode: String,
    pub engine: String,
    pub model: String,
    pub duration_ms: u64,
    pub source_path: String,
    pub has_transcript: bool,
    pub has_audio: bool,
    pub segment_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PruneResult {
    pub sessions_deleted: u64,
    pub segments_deleted: u64,
    pub audio_files_deleted: u64,
}

pub fn save_transcript(
    paths: &ConfigPaths,
    retention: &RetentionConfig,
    transcript: &TranscriptOutput,
    audio_path: Option<&Path>,
) -> Result<String, ComlinkError> {
    fs::create_dir_all(&paths.data_dir)?;
    if retention.audio {
        fs::create_dir_all(&paths.audio_dir)?;
    }

    let connection = open(paths)?;
    let session_id = transcript.session_id.clone();
    let created_at_ms = now_ms();
    let source = if retention.metadata {
        transcript.source.clone()
    } else {
        SourceMetadata {
            path: "<redacted>".to_string(),
            normalized_sample_rate_hz: 0,
            normalized_channels: 0,
        }
    };
    let raw_text = retention
        .transcripts
        .then_some(transcript.raw_text.as_str());
    let final_text = retention
        .transcripts
        .then_some(transcript.final_text.as_str());
    let audio_path = retained_audio_path(paths, retention, audio_path, &session_id)?;

    connection.execute(
        "INSERT INTO sessions (
            id, schema_version, created_at_ms, mode, copied, engine, model, duration_ms, source_path,
            normalized_sample_rate_hz, normalized_channels, raw_text, final_text, audio_path
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            session_id,
            transcript.schema_version,
            created_at_ms,
            transcript.mode.as_str(),
            transcript.copied,
            transcript.engine,
            transcript.model,
            transcript.duration_ms as i64,
            source.path,
            source.normalized_sample_rate_hz as i64,
            source.normalized_channels as i64,
            raw_text,
            final_text,
            audio_path.as_deref(),
        ],
    )?;

    for (index, segment) in transcript.segments.iter().enumerate() {
        let text = retention.transcripts.then_some(segment.text.as_str());
        connection.execute(
            "INSERT INTO segments (
                session_id, segment_index, start_ms, end_ms, text
            ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                index as i64,
                segment.start_ms as i64,
                segment.end_ms as i64,
                text,
            ],
        )?;
    }

    Ok(session_id)
}

pub fn list(paths: &ConfigPaths) -> Result<Vec<StoredSessionSummary>, ComlinkError> {
    let connection = open(paths)?;
    let mut statement = connection.prepare(
        "SELECT
            s.id, s.created_at_ms, s.mode, s.engine, s.model, s.duration_ms, s.source_path,
            s.raw_text IS NOT NULL OR s.final_text IS NOT NULL,
            s.audio_path IS NOT NULL,
            COUNT(g.id)
        FROM sessions s
        LEFT JOIN segments g ON g.session_id = s.id
        GROUP BY s.id
        ORDER BY s.created_at_ms DESC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(StoredSessionSummary {
            id: row.get(0)?,
            created_at_ms: row.get(1)?,
            mode: row.get(2)?,
            engine: row.get(3)?,
            model: row.get(4)?,
            duration_ms: row.get::<_, i64>(5)? as u64,
            source_path: row.get(6)?,
            has_transcript: row.get(7)?,
            has_audio: row.get(8)?,
            segment_count: row.get::<_, i64>(9)? as u64,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(ComlinkError::from)
}

pub fn show(paths: &ConfigPaths, id: &str) -> Result<StoredSession, ComlinkError> {
    let connection = open(paths)?;
    let session = connection
        .query_row(
            "SELECT
                id, schema_version, created_at_ms, mode, copied, engine, model, duration_ms, source_path,
                normalized_sample_rate_hz, normalized_channels, raw_text, final_text, audio_path
            FROM sessions
            WHERE id = ?1",
            params![id],
            |row| {
                Ok(StoredSession {
                    id: row.get(0)?,
                    schema_version: row.get(1)?,
                    created_at_ms: row.get(2)?,
                    mode: row.get(3)?,
                    copied: row.get(4)?,
                    engine: row.get(5)?,
                    model: row.get(6)?,
                    duration_ms: row.get::<_, i64>(7)? as u64,
                    source: SourceMetadata {
                        path: row.get(8)?,
                        normalized_sample_rate_hz: row.get::<_, i64>(9)? as u32,
                        normalized_channels: row.get::<_, i64>(10)? as u16,
                    },
                    raw_text: row.get(11)?,
                    final_text: row.get(12)?,
                    audio_path: row.get(13)?,
                    segments: Vec::new(),
                })
            },
        )
        .optional()?
        .ok_or_else(|| ComlinkError::HistoryNotFound(id.to_string()))?;

    let mut session = session;
    let mut statement = connection.prepare(
        "SELECT start_ms, end_ms, text
        FROM segments
        WHERE session_id = ?1
        ORDER BY segment_index ASC",
    )?;
    let rows = statement.query_map(params![id], |row| {
        Ok(StoredSegment {
            start_ms: row.get::<_, i64>(0)? as u64,
            end_ms: row.get::<_, i64>(1)? as u64,
            text: row.get(2)?,
        })
    })?;
    session.segments = rows.collect::<Result<Vec<_>, _>>()?;

    Ok(session)
}

pub fn prune_all(paths: &ConfigPaths) -> Result<PruneResult, ComlinkError> {
    let mut connection = open(paths)?;
    let transaction = connection.transaction()?;
    let audio_paths = {
        let mut statement =
            transaction.prepare("SELECT audio_path FROM sessions WHERE audio_path IS NOT NULL")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let audio_files_deleted = delete_audio_files(&audio_paths)?;
    let segments_deleted = transaction.execute("DELETE FROM segments", [])? as u64;
    let sessions_deleted = transaction.execute("DELETE FROM sessions", [])? as u64;
    transaction.commit()?;
    Ok(PruneResult {
        sessions_deleted,
        segments_deleted,
        audio_files_deleted,
    })
}

fn open(paths: &ConfigPaths) -> Result<Connection, ComlinkError> {
    if let Some(parent) = paths.database_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(&paths.database_file)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&connection)?;
    Ok(connection)
}

fn migrate(connection: &Connection) -> Result<(), ComlinkError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            schema_version TEXT NOT NULL DEFAULT 'comlink.session.v1',
            created_at_ms INTEGER NOT NULL,
            mode TEXT NOT NULL,
            copied INTEGER NOT NULL DEFAULT 0,
            engine TEXT NOT NULL,
            model TEXT NOT NULL,
            duration_ms INTEGER NOT NULL,
            source_path TEXT NOT NULL,
            normalized_sample_rate_hz INTEGER NOT NULL,
            normalized_channels INTEGER NOT NULL,
            raw_text TEXT,
            final_text TEXT,
            audio_path TEXT
        );

        CREATE TABLE IF NOT EXISTS segments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            segment_index INTEGER NOT NULL,
            start_ms INTEGER NOT NULL,
            end_ms INTEGER NOT NULL,
            text TEXT,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_created_at
            ON sessions(created_at_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_segments_session
            ON segments(session_id, segment_index);",
    )?;
    add_column_if_missing(
        connection,
        "sessions",
        "schema_version",
        "ALTER TABLE sessions ADD COLUMN schema_version TEXT NOT NULL DEFAULT 'comlink.session.v1'",
    )?;
    add_column_if_missing(
        connection,
        "sessions",
        "copied",
        "ALTER TABLE sessions ADD COLUMN copied INTEGER NOT NULL DEFAULT 0",
    )?;
    Ok(())
}

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    alter_sql: &str,
) -> Result<(), ComlinkError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    let exists = columns
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == column);
    if !exists {
        connection.execute(alter_sql, [])?;
    }
    Ok(())
}

fn retained_audio_path(
    paths: &ConfigPaths,
    retention: &RetentionConfig,
    audio_path: Option<&Path>,
    session_id: &str,
) -> Result<Option<String>, ComlinkError> {
    if !retention.audio {
        return Ok(None);
    }
    let Some(audio_path) = audio_path else {
        return Ok(None);
    };
    let extension = audio_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("wav");
    let retained = paths.audio_dir.join(format!("{session_id}.{extension}"));
    fs::copy(audio_path, &retained)?;
    Ok(Some(retained.display().to_string()))
}

fn delete_audio_files(audio_paths: &[String]) -> Result<u64, ComlinkError> {
    let mut deleted = 0;
    for audio_path in audio_paths {
        match fs::remove_file(PathBuf::from(audio_path)) {
            Ok(()) => deleted += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(deleted)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use crate::{
        asr::{Segment, SourceMetadata},
        output::{ContextMetadata, ProcessingStep, TranscriptOutput, SCHEMA_VERSION},
    };

    use super::*;

    fn sample_transcript() -> TranscriptOutput {
        TranscriptOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            session_id: "session-1".to_string(),
            text: "hello".to_string(),
            raw_text: "hello".to_string(),
            final_text: "hello".to_string(),
            mode: "raw".to_string(),
            copied: true,
            engine: "whisper.cpp".to_string(),
            model: "model.bin".to_string(),
            duration_ms: 250,
            segments: vec![Segment {
                start_ms: 0,
                end_ms: 250,
                text: "hello".to_string(),
            }],
            source: SourceMetadata {
                path: "short.wav".to_string(),
                normalized_sample_rate_hz: 16_000,
                normalized_channels: 1,
            },
            context: ContextMetadata::default(),
            processing_steps: vec![ProcessingStep {
                name: "raw".to_string(),
            }],
            warnings: Vec::new(),
            llm: None,
            history_session_id: None,
        }
    }

    #[test]
    fn transcript_retention_can_omit_text_but_keep_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            home_dir: dir.path().to_path_buf(),
            config_file: dir.path().join("config.json"),
            data_dir: dir.path().join("data"),
            database_file: dir.path().join("data/history.sqlite3"),
            audio_dir: dir.path().join("data/audio"),
        };
        let retention = RetentionConfig {
            metadata: true,
            transcripts: false,
            audio: false,
        };

        let id = save_transcript(&paths, &retention, &sample_transcript(), None).unwrap();
        let session = show(&paths, &id).unwrap();

        assert_eq!(session.source.path, "short.wav");
        assert_eq!(session.schema_version, SCHEMA_VERSION);
        assert!(session.copied);
        assert_eq!(session.raw_text, None);
        assert_eq!(session.final_text, None);
        assert_eq!(session.segments[0].text, None);
    }

    #[test]
    fn migration_backfills_contract_fields_for_existing_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            home_dir: dir.path().to_path_buf(),
            config_file: dir.path().join("config.json"),
            data_dir: dir.path().join("data"),
            database_file: dir.path().join("data/history.sqlite3"),
            audio_dir: dir.path().join("data/audio"),
        };
        fs::create_dir_all(&paths.data_dir).unwrap();
        let connection = Connection::open(&paths.database_file).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    created_at_ms INTEGER NOT NULL,
                    mode TEXT NOT NULL,
                    engine TEXT NOT NULL,
                    model TEXT NOT NULL,
                    duration_ms INTEGER NOT NULL,
                    source_path TEXT NOT NULL,
                    normalized_sample_rate_hz INTEGER NOT NULL,
                    normalized_channels INTEGER NOT NULL,
                    raw_text TEXT,
                    final_text TEXT,
                    audio_path TEXT
                );
                INSERT INTO sessions (
                    id, created_at_ms, mode, engine, model, duration_ms, source_path,
                    normalized_sample_rate_hz, normalized_channels, raw_text, final_text, audio_path
                ) VALUES (
                    'old-session', 123, 'memo', 'whisper.cpp', 'model.bin', 250, 'short.wav',
                    16000, 1, 'hello', 'hello', NULL
                );",
            )
            .unwrap();
        drop(connection);

        let session = show(&paths, "old-session").unwrap();

        assert_eq!(session.schema_version, SCHEMA_VERSION);
        assert!(!session.copied);
        assert_eq!(session.final_text.as_deref(), Some("hello"));
    }

    #[test]
    fn prune_all_removes_retained_audio_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            home_dir: dir.path().to_path_buf(),
            config_file: dir.path().join("config.json"),
            data_dir: dir.path().join("data"),
            database_file: dir.path().join("data/history.sqlite3"),
            audio_dir: dir.path().join("data/audio"),
        };
        let retention = RetentionConfig {
            metadata: true,
            transcripts: true,
            audio: true,
        };
        let source_audio = dir.path().join("normalized.wav");
        fs::write(&source_audio, b"audio").unwrap();

        let id = save_transcript(
            &paths,
            &retention,
            &sample_transcript(),
            Some(&source_audio),
        )
        .unwrap();
        let session = show(&paths, &id).unwrap();
        let retained_audio = session.audio_path.unwrap();
        assert!(Path::new(&retained_audio).is_file());

        let result = prune_all(&paths).unwrap();

        assert_eq!(result.sessions_deleted, 1);
        assert_eq!(result.segments_deleted, 1);
        assert_eq!(result.audio_files_deleted, 1);
        assert!(!Path::new(&retained_audio).exists());
        assert!(list(&paths).unwrap().is_empty());
    }
}
