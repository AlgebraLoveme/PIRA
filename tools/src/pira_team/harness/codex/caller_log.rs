//! Native metadata lookup before the bounded legacy rollout-directory fallback.
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{
    fs::{self, File, OpenOptions},
    path::{Component, Path, PathBuf},
    time::Duration,
};

// PIRA: v5 schema verified against installed Codex 0.167.0; other versions use
// the bounded native-layout fallback until a versioned native index is supported.
const STATE_DB: &str = "state_5.sqlite";

enum IndexError {
    Unavailable,
    Invalid(String),
}

impl From<rusqlite::Error> for IndexError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Unavailable
    }
}

pub(super) fn indexed(home: &Path, thread: &str) -> Result<Option<PathBuf>, String> {
    match query_index(home, thread) {
        Ok(path) => Ok(path),
        Err(IndexError::Unavailable) => Ok(None),
        Err(IndexError::Invalid(error)) => Err(error),
    }
}

fn query_index(home: &Path, thread: &str) -> Result<Option<PathBuf>, IndexError> {
    let db = home.join(STATE_DB);
    if !db.try_exists().unwrap_or(false) {
        return Ok(None);
    }
    safe_path(&db).map_err(|_| IndexError::Unavailable)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = home.join(format!("{STATE_DB}{suffix}"));
        if fs::symlink_metadata(&sidecar).is_ok() {
            safe_path(&sidecar).map_err(|_| IndexError::Unavailable)?;
        }
    }
    let mut connection = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(Duration::from_millis(100))?;
    connection.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA,
        false,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    connection.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        16 * 1024 * 1024,
    )?;
    let mut steps = 0;
    connection.progress_handler(
        1000,
        Some(move || {
            steps += 1;
            steps > 1000
        }),
    )?;
    let table: Option<String> = connection
        .query_row(
            "SELECT type FROM sqlite_schema WHERE name = 'threads'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if table.as_deref() != Some("table") {
        return Ok(None);
    }
    let mut statement = connection.prepare("PRAGMA table_info(threads)")?;
    let fields = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(5)?,
        ))
    })?;
    let mut id = false;
    let mut rollout = false;
    let mut primary_keys = 0;
    for field in fields {
        let (name, kind, primary) = field?;
        if primary > 0 {
            primary_keys += 1;
        }
        id |= name == "id" && kind.eq_ignore_ascii_case("TEXT") && primary == 1;
        rollout |= name == "rollout_path" && kind.eq_ignore_ascii_case("TEXT");
    }
    if !id || !rollout || primary_keys != 1 {
        return Err(IndexError::Invalid(
            "native caller index requires a unique thread identity".into(),
        ));
    }
    let mut statement =
        connection.prepare("SELECT rollout_path FROM threads WHERE id = ?1 LIMIT 2")?;
    let mut rows = statement.query([thread])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let path = PathBuf::from(
        row.get::<_, String>(0)
            .map_err(|_| IndexError::Invalid("invalid indexed caller path".into()))?,
    );
    if rows.next()?.is_some() {
        return Err(IndexError::Invalid("ambiguous native caller index".into()));
    }
    if !path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with("rollout-") && name.ends_with(&format!("-{thread}.jsonl"))
        })
    {
        return Err(IndexError::Invalid(
            "native index path does not match caller identity".into(),
        ));
    }
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(IndexError::Invalid("invalid indexed caller path".into()));
    }
    // Resolve only this index-pinned parent, never symlink-walk a session tree.
    // The final transcript itself must remain a regular non-symlink file.
    let parent = path
        .parent()
        .ok_or_else(|| IndexError::Invalid("invalid indexed caller parent".into()))?
        .canonicalize()
        .map_err(|_| IndexError::Invalid("indexed caller parent unavailable".into()))?;
    let physical = parent.join(path.file_name().unwrap());
    safe_path(&physical).map_err(IndexError::Invalid)?;
    Ok(Some(physical))
}

pub(super) fn open(path: &Path) -> Result<File, String> {
    safe_path(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| "cannot open pinned caller transcript")?;
    regular(
        &file
            .metadata()
            .map_err(|_| "cannot inspect pinned caller transcript")?,
    )?;
    Ok(file)
}

fn regular(metadata: &fs::Metadata) -> Result<(), String> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("caller metadata must be a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err("caller metadata must be user-owned and not group/world writable".into());
        }
    }
    Ok(())
}

fn safe_path(path: &Path) -> Result<(), String> {
    regular(&metadata_without_links(path)?)
}

pub(super) fn directory(path: &Path) -> Result<(), String> {
    if !metadata_without_links(path)?.is_dir() {
        return Err("caller session path must be a directory".into());
    }
    Ok(())
}

fn metadata_without_links(path: &Path) -> Result<fs::Metadata, String> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err("caller metadata paths must be absolute without traversal".into());
    }
    // Inspect parents before children, without descending through an observed link.
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let metadata =
            fs::symlink_metadata(ancestor).map_err(|_| "caller metadata path unavailable")?;
        if metadata.file_type().is_symlink() {
            return Err("caller metadata paths must not traverse symlinks".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("caller metadata paths must not traverse reparse points".into());
            }
        }
        if ancestor == path {
            return Ok(metadata);
        }
    }
    Err("caller metadata path unavailable".into())
}
