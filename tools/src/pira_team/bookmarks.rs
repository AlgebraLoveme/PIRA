//! Store-wide management pointers; never part of worker inputs or configuration.
use crate::{lifecycle, profile::defaults, storage};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const FILE_NAME: &str = "bookmarks.json";
const MAX_BYTES: u64 = 1024 * 1024;
const LOCK_WAIT_BUDGET: Duration = Duration::from_secs(5);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(20);
type Index = BTreeMap<String, String>;

pub fn validate_label(label: &str) -> Result<(), String> {
    if label.is_empty()
        || label.len() > 128
        || label.trim() != label
        || label.chars().any(char::is_control)
        || label.contains(['/', '\\'])
        || label.starts_with('@')
        || matches!(label, "." | "..")
    {
        return Err("label must be 1..128 UTF-8 bytes, without edge whitespace, controls, / or \\; no leading @ or . / .. names".into());
    }
    Ok(())
}

fn serializable_root(root: PathBuf) -> Result<PathBuf, String> {
    if root.to_str().is_none() {
        return Err("bookmark store's physical path must be UTF-8 for JSON run references".into());
    }
    Ok(root)
}

fn root_if_present(root: &Path) -> Result<Option<PathBuf>, String> {
    defaults::root_if_present(root)?
        .map(serializable_root)
        .transpose()
}

fn open(path: &Path, write: bool) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true).write(write).create(write);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("open bookmark metadata: {e}"))?;
    defaults::inspect_managed_file(&file.metadata().map_err(|e| e.to_string())?, "bookmarks")?;
    Ok(file)
}

fn inspect_if_present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            defaults::inspect_managed_file(&metadata, "bookmarks")?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("inspect bookmark metadata: {e}")),
    }
}

fn load(root: &Path) -> Result<Index, String> {
    let path = root.join(FILE_NAME);
    load_file(&path).map_err(|e| {
        format!(
            "invalid/unreadable {}: {e}; restore the index before retrying",
            path.display()
        )
    })
}

fn load_file(path: &Path) -> Result<Index, String> {
    if !inspect_if_present(path)? {
        return Ok(Index::new());
    }
    let mut bytes = Vec::new();
    open(path, false)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read bookmarks: {e}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("bookmark index exceeds 1 MiB".into());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid bookmark JSON")?;
    // Arrays avoid JSON object duplicate-key overwrite. Versioned format is
    // [1, [[label, literal_run_id], ...]]; duplicate labels fail, never silently drop.
    let document = value
        .as_array()
        .filter(|v| v.len() == 2 && v[0] == 1)
        .ok_or("invalid bookmark index schema")?;
    let entries = document[1].as_array().ok_or("invalid bookmark entries")?;
    let mut index = Index::new();
    for entry in entries {
        let pair = entry
            .as_array()
            .filter(|v| v.len() == 2)
            .ok_or("invalid bookmark entry")?;
        let label = pair[0].as_str().ok_or("invalid bookmark label")?;
        let id = pair[1].as_str().ok_or("invalid bookmark run ID")?;
        validate_label(label).map_err(|_| "invalid stored bookmark label")?;
        storage::validate_run_id(id).map_err(|_| "invalid stored bookmark run ID")?;
        if index.insert(label.to_owned(), id.to_owned()).is_some() {
            return Err("duplicate label in bookmark index".into());
        }
    }
    Ok(index)
}

fn lock(root: &Path) -> Result<File, String> {
    let path = root.join("bookmarks.lock");
    inspect_if_present(&path)?;
    let file = open(&path, true)?;
    // Stable handle is retained across contention retries. This budget covers only
    // metadata lock acquisition, never worker startup or inference.
    let start = Instant::now();
    loop {
        if crate::CANCELLED.load(Ordering::SeqCst) {
            return Err("interrupted while waiting for bookmark lock".into());
        }
        let remaining = LOCK_WAIT_BUDGET.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err("bookmarks are busy after waiting 5 seconds; retry when the other metadata edit finishes".into());
        }
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                std::thread::sleep(LOCK_RETRY_INTERVAL.min(remaining));
            }
            Err(error) => {
                return Err(format!(
                    "bookmarks lock is unavailable: {error}; check store permissions and lock support"
                ));
            }
        }
    }
    // Cancellation racing with successful acquisition must not enter an update.
    if crate::CANCELLED.load(Ordering::SeqCst) {
        return Err("interrupted while waiting for bookmark lock".into());
    }
    Ok(file)
}

fn persist(root: &Path, index: &Index) -> Result<(), String> {
    let entries: Vec<_> = index.iter().map(|(label, id)| [label, id]).collect();
    let mut bytes = serde_json::to_vec(&json!([1, entries])).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_BYTES {
        return Err("bookmark index would exceed 1 MiB; prior file was not changed".into());
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| format!("bookmark temporary name: {e}"))?;
    let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = root.join(format!(".bookmarks-{name}.tmp"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|e| format!("create bookmark temporary file: {e}"))?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|e| format!("write bookmarks: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("sync bookmarks: {e}"))?;
        drop(file);
        fs::rename(&temporary, root.join(FILE_NAME))
            .map_err(|e| format!("replace bookmarks: {e}"))?;
        #[cfg(unix)]
        File::open(root)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| format!("bookmarks replaced but directory sync failed: {e}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn run_reference(root: &Path, id: &str) -> Value {
    json!({"run_id":id,"run_root":root.join(id)})
}

fn lookup(index: &Index, label: &str) -> Result<String, String> {
    validate_label(label)?;
    index
        .get(label)
        .cloned()
        .ok_or_else(|| format!("bookmark {label:?} not found"))
}

/// Resolve exactly one label from an atomic snapshot; mappings contain literal IDs only.
pub fn resolve(root: &Path, label: &str) -> Result<String, String> {
    validate_label(label)?;
    let root = root_if_present(root)?.ok_or("Team store does not exist")?;
    lookup(&load(&root)?, label)
}

/// Add/move a unique label under one update lock; aliases use that same snapshot.
pub fn assign(root: &Path, reference: &str, label: &str) -> Result<Value, String> {
    validate_label(label)?;
    let root = root_if_present(root)?.ok_or("Team store does not exist")?;
    let _lock = lock(&root)?;
    let mut index = load(&root)?;
    let id = if let Some(alias) = reference.strip_prefix('@') {
        lookup(&index, alias)?
    } else {
        storage::validate_run_id(reference)?;
        reference.to_owned()
    };
    let run = storage::locate(&root, &id)?;
    lifecycle::private_path(&run, true)?;
    lifecycle::private_path(&run.join("manifest.json"), false)?;
    let previous = index.get(label).cloned();
    let changed = previous.as_deref() != Some(&id);
    if changed {
        index.insert(label.to_owned(), id.clone());
        persist(&root, &index)?;
    }
    Ok(
        json!({"label":label,"previous":previous.map(|id| run_reference(&root,&id)),
        "current":run_reference(&root,&id),"changed":changed}),
    )
}

fn remove(root: &Path, label: &str) -> Result<Value, String> {
    validate_label(label)?;
    let Some(root) = root_if_present(root)? else {
        return Ok(json!({"label":label,"previous":null,"current":null,"changed":false}));
    };
    let _lock = lock(&root)?;
    let mut index = load(&root)?;
    let previous = index.remove(label);
    if previous.is_some() {
        persist(&root, &index)?;
    }
    Ok(
        json!({"label":label,"previous":previous.as_deref().map(|id|run_reference(&root,id)),
        "current":null,"changed":previous.is_some()}),
    )
}

fn browse(root: &Path, query: Option<&str>, limit: usize) -> Result<Value, String> {
    let Some(root) = root_if_present(root)? else {
        return Ok(json!({"matches":[],"has_more":false}));
    };
    // Validate the whole index before limiting. No manifests, logs or sessions are read.
    let index = load(&root)?;
    let query = query.map(str::to_lowercase);
    let mut matches = Vec::new();
    let mut has_more = false;
    for (label, id) in index {
        if query
            .as_ref()
            .is_some_and(|q| !label.to_lowercase().contains(q))
        {
            continue;
        }
        if matches.len() == limit {
            has_more = true;
            break;
        }
        matches.push(json!({"label":label,"run_id":id,"run_root":root.join(&id)}));
    }
    Ok(json!({"matches":matches,"has_more":has_more}))
}

/// Session-free bookmark/search/list/remove management; no worker launch or auth lookup.
pub fn command(args: &[String]) -> Result<(), String> {
    let action = args[0].as_str();
    let (mut store, mut label, mut limit, mut target) = (None, None, None, None);
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            crate::set_once(
                &mut target,
                rest.next().ok_or("missing target after --")?.clone(),
                "target",
            )?;
            if rest.next().is_some() {
                return Err("provide exactly one target".into());
            }
            break;
        }
        if !arg.starts_with('-') {
            crate::set_once(&mut target, arg.clone(), "target")?;
            continue;
        }
        let slot = match arg.as_str() {
            "--store" => &mut store,
            "--label" if action == "bookmark" => &mut label,
            "--limit" if matches!(action, "search" | "bookmarks") => &mut limit,
            _ => return Err(format!("unknown {action} option {arg}")),
        };
        crate::set_once(
            slot,
            rest.next()
                .ok_or_else(|| format!("missing value for {arg}"))?
                .clone(),
            arg,
        )?;
    }
    if store.as_deref() == Some("") {
        return Err("--store must not be empty".into());
    }
    let limit = limit
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|_| "--limit must be an integer from 1 through 1000")?
        .unwrap_or(20);
    if !(1..=1000).contains(&limit) {
        return Err("--limit must be from 1 through 1000".into());
    }
    match action {
        "bookmark" => {
            validate_label(label.as_deref().ok_or("bookmark requires --label LABEL")?)?;
            let reference = target
                .as_deref()
                .ok_or("bookmark requires RUN_ID or @LABEL")?;
            if let Some(alias) = reference.strip_prefix('@') {
                validate_label(alias)?;
            } else {
                storage::validate_run_id(reference)?;
            }
        }
        "unbookmark" => validate_label(target.as_deref().ok_or("unbookmark requires LABEL")?)?,
        "bookmarks" if target.is_some() => {
            return Err("bookmarks takes no positional arguments".into());
        }
        "search" => {
            let query = target.as_deref().ok_or("search requires QUERY")?;
            if query.trim().is_empty() || query.len() > 128 || query.chars().any(char::is_control) {
                return Err(
                    "query must be nonblank, without controls, and at most 128 UTF-8 bytes".into(),
                );
            }
        }
        _ => {}
    }
    let root = store
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(storage::default_root)?;
    let result = match action {
        "bookmark" => assign(&root, target.as_deref().unwrap(), label.as_deref().unwrap())?,
        "unbookmark" => remove(&root, target.as_deref().unwrap())?,
        "bookmarks" => browse(&root, None, limit)?,
        "search" => browse(&root, target.as_deref(), limit)?,
        _ => unreachable!(),
    };
    println!("{result}");
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn physical_store_paths_must_be_serializable_before_any_index_update() {
        let invalid = PathBuf::from(OsString::from_vec(b"/store-\xff".to_vec()));
        assert!(serde_json::to_value(&invalid).is_err());
        assert!(serializable_root(invalid).is_err());
        let valid = PathBuf::from("/store-λ");
        assert_eq!(serializable_root(valid.clone()).unwrap(), valid);
    }
}
