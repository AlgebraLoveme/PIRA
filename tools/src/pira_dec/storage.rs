use crate::model::{self, DecisionDraft, DecisionRecord, MAX_RECORD_BYTES};
use crate::util;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Clone, Debug)]
pub struct Layout {
    pub workspace_dir: PathBuf,
    pub records_dir: PathBuf,
    pub temporary_dir: PathBuf,
    pub lock_path: PathBuf,
}

impl Layout {
    pub fn current(store_option: Option<&Path>) -> Result<Self, String> {
        let root = effective_store_dir(store_option)?;
        let workspace_hash = current_workspace_hash()?;
        let workspace_dir = root.join(workspace_hash);
        Ok(Self {
            records_dir: workspace_dir.join("records"),
            temporary_dir: workspace_dir.join(".tmp"),
            lock_path: workspace_dir.join(".write.lock"),
            workspace_dir,
        })
    }

    pub fn prepare_for_write(&self) -> Result<(), String> {
        let root = self
            .workspace_dir
            .parent()
            .ok_or_else(|| "decision workspace has no store root".to_string())?;
        ensure_private_dir(root)?;
        ensure_private_dir(&self.workspace_dir)?;
        ensure_private_dir(&self.records_dir)?;
        ensure_private_dir(&self.temporary_dir)?;
        reject_symlink_if_present(&self.lock_path, "decision write lock")?;
        Ok(())
    }

    pub fn records_available(&self) -> Result<bool, String> {
        let Some(root) = self.workspace_dir.parent() else {
            return Err("decision workspace has no store root".into());
        };
        if !real_directory(root, "decision store")? {
            return Ok(false);
        }
        if !real_directory(&self.workspace_dir, "decision workspace")? {
            return Ok(false);
        }
        real_directory(&self.records_dir, "decision records")
    }
}

pub struct WriteLock {
    _file: File,
}

impl WriteLock {
    pub fn acquire(layout: &Layout) -> Result<Self, String> {
        reject_symlink_if_present(&layout.lock_path, "decision write lock")?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&layout.lock_path)
            .map_err(|error| format!("open {}: {error}", layout.lock_path.display()))?;
        file.lock()
            .map_err(|error| format!("lock {}: {error}", layout.lock_path.display()))?;
        Ok(Self { _file: file })
    }
}

#[derive(Debug)]
pub enum ReadFailure {
    Vanished,
    Invalid(String),
}

#[derive(Debug)]
pub enum Resolution {
    Missing,
    Ambiguous,
    Found(PathBuf),
}

pub fn add(store_option: Option<&Path>, draft: DecisionDraft) -> Result<DecisionRecord, String> {
    let draft = draft.normalized()?;
    let layout = Layout::current(store_option)?;
    layout.prepare_for_write()?;
    loop {
        let timestamp_ms = util::now_ms()?;
        let id = util::decision_id(timestamp_ms)?;
        let record = DecisionRecord::from_draft(id.clone(), timestamp_ms, &draft)?;
        let bytes = model::encode(&record)?;
        let temporary = layout.temporary_dir.join(format!(
            "{id}-{}-{}.tmp",
            std::process::id(),
            util::nonce_hex()
        ));
        write_temporary(&temporary, &bytes)?;
        let lock = match WriteLock::acquire(&layout) {
            Ok(lock) => lock,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        let final_path = layout.records_dir.join(format!("{id}.piradec"));
        match fs::symlink_metadata(&final_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                drop(lock);
                let _ = fs::remove_file(&temporary);
                return Err(format!(
                    "refusing symlinked decision record {}",
                    final_path.display()
                ));
            }
            Ok(_) => {
                drop(lock);
                let _ = fs::remove_file(&temporary);
                continue;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                drop(lock);
                let _ = fs::remove_file(&temporary);
                return Err(error.to_string());
            }
        }
        if let Err(error) = publish_no_clobber(&temporary, &final_path) {
            drop(lock);
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        sync_directory(&layout.records_dir).map_err(|error| {
            format!("published decision {id} but could not sync records directory: {error}")
        })?;
        drop(lock);
        return Ok(record);
    }
}

pub fn record_paths(layout: &Layout) -> Result<Vec<PathBuf>, String> {
    if !layout.records_available()? {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for item in fs::read_dir(&layout.records_dir)
        .map_err(|error| format!("read {}: {error}", layout.records_dir.display()))?
    {
        let path = item.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("piradec") {
            paths.push(path);
        }
    }
    Ok(paths)
}

pub fn resolve(layout: &Layout, query: &str, exact: bool) -> Result<Resolution, String> {
    let mut matches = Vec::new();
    for path in record_paths(layout)? {
        let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if (exact && id == query) || (!exact && id.starts_with(query)) {
            matches.push(path);
        }
    }
    matches.sort();
    Ok(match matches.len() {
        0 => Resolution::Missing,
        1 => Resolution::Found(matches.remove(0)),
        _ => Resolution::Ambiguous,
    })
}

pub fn read_record(path: &Path) -> Result<DecisionRecord, ReadFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Err(ReadFailure::Vanished),
        Err(error) => return Err(ReadFailure::Invalid(error.to_string())),
    };
    if metadata.file_type().is_symlink() {
        return Err(ReadFailure::Invalid(
            "refusing symlinked decision record".into(),
        ));
    }
    if !metadata.is_file() {
        return Err(ReadFailure::Invalid("decision record is not a file".into()));
    }
    if metadata.len() > MAX_RECORD_BYTES as u64 {
        return Err(ReadFailure::Invalid(
            "decision record exceeds size limit".into(),
        ));
    }
    let file = File::open(path).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            ReadFailure::Vanished
        } else {
            ReadFailure::Invalid(error.to_string())
        }
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| ReadFailure::Invalid(error.to_string()))?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(ReadFailure::Invalid(
            "decision record exceeds size limit".into(),
        ));
    }
    let record = model::decode(&bytes).map_err(ReadFailure::Invalid)?;
    let expected = format!("{}.piradec", record.id);
    if path.file_name().and_then(|value| value.to_str()) != Some(expected.as_str()) {
        return Err(ReadFailure::Invalid(
            "decision filename does not match embedded ID".into(),
        ));
    }
    Ok(record)
}

pub fn delete_exact(layout: &Layout, id: &str) -> Result<Option<DecisionRecord>, String> {
    if !layout.records_available()? {
        return Ok(None);
    }
    let path = layout.records_dir.join(format!("{id}.piradec"));
    let before = match read_record(&path) {
        Ok(record) => record,
        Err(ReadFailure::Vanished) => return Ok(None),
        Err(ReadFailure::Invalid(error)) => return Err(error),
    };
    let _lock = WriteLock::acquire(layout)?;
    let current = match read_record(&path) {
        Ok(record) => record,
        Err(ReadFailure::Vanished) => return Ok(None),
        Err(ReadFailure::Invalid(error)) => return Err(error),
    };
    if before != current {
        return Err("decision record changed before deletion".into());
    }
    fs::remove_file(&path).map_err(|error| format!("delete {}: {error}", path.display()))?;
    sync_directory(&layout.records_dir).map_err(|error| {
        format!(
            "deleted decision {} but could not sync records directory: {error}",
            current.id
        )
    })?;
    Ok(Some(current))
}

fn write_temporary(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(error.to_string());
    }
    Ok(())
}

fn publish_no_clobber(temporary: &Path, final_path: &Path) -> Result<(), String> {
    match fs::hard_link(temporary, final_path) {
        Ok(()) => {
            if fs::remove_file(temporary).is_err() {
                eprintln!(
                    "pira_dec: warning: decision was published but its temporary link remains"
                );
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(format!(
            "decision path already exists: {}",
            final_path.display()
        )),
        Err(_) if !final_path.exists() => {
            fs::rename(temporary, final_path).map_err(|error| error.to_string())
        }
        Err(error) => Err(format!("publish {}: {error}", final_path.display())),
    }
}

fn effective_store_dir(option: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = option {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("PIRA_DEC_STORE_DIR") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("PIRA").join("decision"));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("PIRA")
            .join("decision"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(path).join("pira").join("decision"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("pira")
                .join("decision"));
        }
    }
    Err("cannot determine a per-user pira_dec store; set PIRA_DEC_STORE_DIR or --store-dir".into())
}

fn current_workspace_hash() -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let root = nearest_git_root(&cwd).unwrap_or(cwd);
    let identity = root.canonicalize().unwrap_or(root).display().to_string();
    let digest = Sha256::digest(identity.as_bytes());
    Ok(util::hex(&digest[..8]))
}

fn nearest_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(path) = current {
        if path.join(".git").exists() {
            return Some(path.to_path_buf());
        }
        current = path.parent();
    }
    None
}

fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!("refusing symlinked directory {}", path.display()));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(format!("path is not a directory: {}", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .map_err(|error| format!("create {}: {error}", path.display()))?;
        }
        Err(error) => return Err(error.to_string()),
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("chmod {}: {error}", path.display()))?;
    Ok(())
}

fn real_directory(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("refusing symlinked {label}: {}", path.display()))
        }
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(format!("{label} is not a directory: {}", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn reject_symlink_if_present(path: &Path, label: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("refusing symlinked {label}: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}
