use crate::model::{self, DecisionDraft, DecisionRecord, MAX_RECORD_BYTES};
use crate::util;
use sha2::{Digest, Sha256};
#[cfg(not(windows))]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
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
    legacy_dir: Option<PathBuf>,
}

impl Layout {
    pub fn current(store_option: Option<&Path>) -> Result<Self, String> {
        let root = effective_store_dir(store_option)?;
        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        let anchor = std::env::var_os("PIRA_DEC_WORKSPACE_DIR").map(PathBuf::from);
        let workspace = workspace_for_cwd(&cwd, anchor.as_deref())?;
        Self::for_workspace(&root, &workspace)
    }

    fn for_workspace(root: &Path, workspace: &Path) -> Result<Self, String> {
        let root = std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(root);
        let (workspace_hash, legacy_hash) = workspace_identity(workspace);
        let workspace_dir = root.join(workspace_hash);
        let layout = Self {
            records_dir: workspace_dir.join("records"),
            temporary_dir: workspace_dir.join(".tmp"),
            lock_path: workspace_dir.join(".write.lock"),
            legacy_dir: legacy_hash.map(|hash| root.join(hash)),
            workspace_dir,
        };
        layout.check_legacy()?;
        Ok(layout)
    }

    fn check_legacy(&self) -> Result<(), String> {
        let Some(legacy) = &self.legacy_dir else {
            return Ok(());
        };
        let root = legacy.parent().ok_or("legacy store has no parent")?;
        if !real_directory(root, "decision store")?
            || !real_directory(legacy, "legacy decision workspace")?
            || !real_directory(&legacy.join("records"), "legacy decision records")?
        {
            return Ok(());
        }
        for entry in fs::read_dir(legacy.join("records")).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|ext| ext == "piradec") {
                return Err(format!(
                    "ambiguous legacy decision records in {}; refusing to mix or hide them. Stop old-version writers, back up the store, and manually attribute and migrate records to {} (or their actual workspace). No automatic migration is performed",
                    legacy.display(),
                    self.workspace_dir.display()
                ));
            }
        }
        Ok(())
    }

    pub fn prepare_for_write(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            self.prepare_with_sync(sync_directory)
        }
        // Windows promises private creation and flushed file contents, not
        // crash persistence of namespace entries. Do not fake a sync barrier.
        #[cfg(not(unix))]
        {
            self.prepare_private_directories()
        }
    }

    fn prepare_private_directories(&self) -> Result<(), String> {
        util::require_write_support()?;
        // Validate all destinations before any directory creation or chmod.
        for path in [&self.records_dir, &self.temporary_dir, &self.lock_path] {
            reject_symlink_if_present(path, "decision store path component")?;
        }
        self.check_legacy()?;
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

    #[cfg(unix)]
    fn prepare_with_sync(
        &self,
        mut sync: impl FnMut(&Path) -> Result<(), String>,
    ) -> Result<(), String> {
        self.prepare_private_directories()?;
        // Sync the entire chain even on retry: an earlier attempt may have
        // created a directory and then failed before syncing its parent link.
        sync(&self.records_dir)?;
        sync(&self.temporary_dir)?;
        for ancestor in self.workspace_dir.ancestors() {
            sync(ancestor)?;
        }
        Ok(())
    }

    pub fn records_available(&self) -> Result<bool, String> {
        self.check_legacy()?;
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
        util::require_write_support()?;
        reject_symlink_if_present(&layout.lock_path, "decision write lock")?;
        #[cfg(windows)]
        let opened = crate::windows::open_lock(&layout.lock_path);
        #[cfg(not(windows))]
        let opened = {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true);
            #[cfg(unix)]
            options.mode(0o600);
            options.open(&layout.lock_path)
        };
        let file =
            opened.map_err(|error| format!("open {}: {error}", layout.lock_path.display()))?;
        util::check_private_file(&file)?;
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
        util::write_private_new(&temporary, &bytes)?;
        let lock = match WriteLock::acquire(&layout) {
            Ok(lock) => lock,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        // Validate against the same locked state in which the record is published.
        if let Err(error) = layout
            .check_legacy()
            .and_then(|()| validate_relationship_targets(&layout, &draft))
        {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        let final_path = layout.records_dir.join(format!("{id}.piradec"));
        match fs::symlink_metadata(&final_path) {
            Ok(metadata) if is_link(&metadata) => {
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
        #[cfg(unix)]
        sync_directory(&layout.records_dir).map_err(|error| {
            format!("published decision {id} but could not sync records directory: {error}")
        })?;
        drop(lock);
        return Ok(record);
    }
}

fn validate_relationship_targets(layout: &Layout, draft: &DecisionDraft) -> Result<(), String> {
    for id in draft.supersedes.iter().chain(draft.related.iter()) {
        model::validate_id_syntax(id)?;
        let path = match resolve(layout, id, true)? {
            Resolution::Found(path) => path,
            Resolution::Missing => return Err(format!("related decision {id} does not exist")),
            Resolution::Ambiguous => {
                return Err(format!("related decision ID {id} is ambiguous"));
            }
        };
        read_record(&path).map_err(|error| match error {
            ReadFailure::Vanished => format!("related decision {id} vanished"),
            ReadFailure::Invalid(error) => format!("related decision {id} is invalid: {error}"),
        })?;
    }
    Ok(())
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
    if exact || model::validate_id_syntax(query).is_ok() {
        model::validate_id_syntax(query)?;
        if !layout.records_available()? {
            return Ok(Resolution::Missing);
        }
        let path = layout.records_dir.join(format!("{query}.piradec"));
        return match fs::symlink_metadata(&path) {
            Ok(_) => Ok(Resolution::Found(path)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Resolution::Missing),
            Err(error) => Err(format!("inspect decision {query}: {error}")),
        };
    }
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
    if is_link(&metadata) {
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
    util::require_write_support()?;
    if !layout.records_available()? {
        return Ok(None);
    }
    for directory in [
        layout.workspace_dir.parent().ok_or("missing store root")?,
        &layout.workspace_dir,
        &layout.records_dir,
    ] {
        let file = open_directory(directory)
            .map_err(|error| format!("open {}: {error}", directory.display()))?;
        util::check_private_file(&file)?;
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
    #[cfg(unix)]
    sync_directory(&layout.records_dir).map_err(|error| {
        format!(
            "deleted decision {} but could not sync records directory: {error}",
            current.id
        )
    })?;
    Ok(Some(current))
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
        Err(error) => Err(format!(
            "cannot atomically publish {} using a hard link: {error}; use a decision store on a filesystem that permits hard links; no rename fallback is used",
            final_path.display()
        )),
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
        if let Some(path) = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            return Ok(path.join("pira").join("decision"));
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

fn workspace_identity(root: &Path) -> (String, Option<String>) {
    let legacy_digest = Sha256::digest(root.to_string_lossy().as_bytes());
    let legacy = util::hex(&legacy_digest[..8]);
    if root.to_str().is_some_and(|text| !text.contains('\u{fffd}')) {
        return (legacy, None);
    }
    #[cfg(unix)]
    let native = {
        use std::os::unix::ffi::OsStrExt;
        root.as_os_str().as_bytes().to_vec()
    };
    #[cfg(windows)]
    let native: Vec<u8> = {
        use std::os::windows::ffi::OsStrExt;
        root.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    };
    #[cfg(not(any(unix, windows)))]
    let native = root.as_os_str().as_encoded_bytes().to_vec();
    let digest = Sha256::digest(&native);
    (format!("native-v1-{}", util::hex(&digest)), Some(legacy))
}

fn workspace_for_cwd(cwd: &Path, anchor: Option<&Path>) -> Result<PathBuf, String> {
    let cwd = cwd
        .canonicalize()
        .map_err(|error| format!("decision cwd: {error}"))?;
    let anchor = anchor
        .map(|path| -> Result<PathBuf, String> {
            if !path.is_absolute() {
                return Err("PIRA_DEC_WORKSPACE_DIR must be an absolute directory".into());
            }
            let physical = path
                .canonicalize()
                .map_err(|error| format!("PIRA_DEC_WORKSPACE_DIR: {error}"))?;
            if physical != path {
                return Err("PIRA_DEC_WORKSPACE_DIR must remain a canonical directory".into());
            }
            if !physical.is_dir() {
                return Err("PIRA_DEC_WORKSPACE_DIR must be a directory".into());
            }
            Ok(physical)
        })
        .transpose()?;
    let git = nearest_git_root(&cwd);
    // Physical ancestry prevents symlink escapes; Git identity prevents crossing
    // into nested repositories. Outside that scope retain ordinary cwd scoping.
    if let Some(anchor) = anchor
        && cwd.starts_with(&anchor)
        && nearest_git_root(&anchor) == git
    {
        return Ok(git.unwrap_or(anchor));
    }
    Ok(git.unwrap_or(cwd))
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
    reject_symlink_if_present(path, "decision store path component")?;
    create_private_directories(path)?;
    let directory =
        open_directory(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    util::check_private_file(&directory)?;
    #[cfg(unix)]
    directory
        .set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("chmod {}: {error}", path.display()))?;
    Ok(())
}

fn create_private_directories(path: &Path) -> Result<(), String> {
    if real_directory(path, "decision directory")? {
        return Ok(());
    }
    let parent = path.parent().ok_or("decision directory has no parent")?;
    create_private_directories(parent)?;
    #[cfg(windows)]
    let created = crate::windows::create_directory(path);
    #[cfg(not(windows))]
    let created = {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path)
    };
    match created {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            if !real_directory(path, "decision directory")? {
                return Err(format!("directory vanished: {}", path.display()));
            }
        }
        Err(error) => return Err(format!("create {}: {error}", path.display())),
    }
    let directory =
        open_directory(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    util::check_private_file(&directory)?;
    Ok(())
}

fn real_directory(path: &Path, label: &str) -> Result<bool, String> {
    reject_symlink_if_present(path, "decision store path component")?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) => Err(format!(
            "refusing symlinked or reparse {label}: {}",
            path.display()
        )),
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(format!("{label} is not a directory: {}", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn reject_symlink_if_present(path: &Path, label: &str) -> Result<(), String> {
    let mut prefix = PathBuf::new();
    let mut missing_prefix = false;
    // Walk lexical components: canonicalization or cancellation of `..` would
    // conceal links. This guards stable paths, not concurrent replacements.
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        if missing_prefix && component == std::path::Component::ParentDir {
            return Err(format!(
                "refusing parent traversal after missing decision store path component: {}",
                prefix.display()
            ));
        }
        prefix.push(component.as_os_str());
        // A rooted Windows prefix is not a directory until RootDir is added:
        // probing `\\?\C:` alone targets the volume, not `\\?\C:\`.
        // Do not canonicalize: every subsequent lexical component (including
        // those before `..`) must still receive the normal link/reparse check.
        if matches!(component, std::path::Component::Prefix(_))
            && matches!(components.peek(), Some(std::path::Component::RootDir))
        {
            continue;
        }
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(format!(
                    "refusing symlinked or reparse {label}: {}",
                    prefix.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => missing_prefix = true,
            Err(error) => {
                return Err(format!("inspect {label} {}: {error}", prefix.display()));
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn open_directory(path: &Path) -> std::io::Result<File> {
    crate::windows::open_directory(path)
}

#[cfg(not(windows))]
fn open_directory(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("sync directory {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sandbox(PathBuf);
    impl Sandbox {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("pira-dec-storage-{}", util::nonce_hex()));
            fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_rooted_prefix_checks_start_at_directory_root() {
        let sandbox = Sandbox::new();
        let root = sandbox.0.ancestors().last().unwrap();
        // canonicalize() produces verbatim Windows paths. Neither the complete
        // root nor an existing/missing descendant may probe the bare volume.
        assert!(matches!(
            root.components().next(),
            Some(std::path::Component::Prefix(_))
        ));
        for path in [
            root.to_path_buf(),
            sandbox.0.clone(),
            sandbox.0.join("missing/child"),
        ] {
            reject_symlink_if_present(&path, "fixture").unwrap();
        }
        // Also cover the ordinary drive-root spelling without assuming C:.
        if let Some(std::path::Component::Prefix(prefix)) = root.components().next() {
            if let std::path::Prefix::VerbatimDisk(drive) = prefix.kind() {
                let ordinary_root = PathBuf::from(format!(r"{}:\", char::from(drive)));
                reject_symlink_if_present(&ordinary_root, "fixture").unwrap();
            }
        }
        // PathBuf::join normalizes `..` when its base is verbatim. Keep the
        // actual lexical input intact so this tests the guard, not normalization.
        let mut lexical = sandbox.0.as_os_str().to_os_string();
        lexical.push(r"\missing\..\other");
        let error = reject_symlink_if_present(Path::new(&lexical), "fixture").unwrap_err();
        assert!(error.contains("parent traversal after missing"), "{error}");
        assert!(!sandbox.0.join("missing").exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_publication_and_owner_lock_without_namespace_durability_claim() {
        let sandbox = Sandbox::new();
        let layout = Layout::for_workspace(&sandbox.0.join("new/store"), &sandbox.0).unwrap();
        // Exercise production setup; this does not simulate power loss.
        layout.prepare_for_write().unwrap();
        let lock = WriteLock::acquire(&layout).unwrap();
        let contender = crate::windows::open_lock(&layout.lock_path).unwrap();
        assert!(matches!(
            contender.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(lock);
        contender.try_lock().unwrap();
        drop(contender);
        let temporary = layout.temporary_dir.join("record.tmp");
        let published = layout.records_dir.join("record.piradec");
        util::write_private_new(&temporary, b"complete bytes").unwrap();
        publish_no_clobber(&temporary, &published).unwrap();
        assert!(!temporary.exists());
        util::check_private_file(&File::open(&published).unwrap()).unwrap();
        assert_eq!(fs::read(&published).unwrap(), b"complete bytes");
        util::write_private_new(&temporary, b"replacement").unwrap();
        assert!(publish_no_clobber(&temporary, &published).is_err());
        assert_eq!(fs::read(&published).unwrap(), b"complete bytes");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn new_ancestors_are_private_and_existing_ancestors_unchanged() {
        let sandbox = Sandbox::new();
        fs::set_permissions(&sandbox.0, fs::Permissions::from_mode(0o755)).unwrap();
        let root = sandbox.0.join("new/ancestor/store");
        let layout = Layout::for_workspace(&root, &sandbox.0).unwrap();
        layout.prepare_for_write().unwrap();
        for path in [
            sandbox.0.join("new"),
            sandbox.0.join("new/ancestor"),
            root,
            layout.workspace_dir,
            layout.records_dir,
            layout.temporary_dir,
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert_eq!(
            fs::metadata(&sandbox.0).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn ancestor_sync_failure_is_visible_and_retry_resyncs_existing_chain() {
        let sandbox = Sandbox::new();
        let root = sandbox.0.join("new/store");
        let layout = Layout::for_workspace(&root, &sandbox.0).unwrap();
        let error = layout
            .prepare_with_sync(|path| {
                if path == sandbox.0 {
                    Err("injected parent sync failure".into())
                } else {
                    sync_directory(path)
                }
            })
            .unwrap_err();
        assert_eq!(error, "injected parent sync failure");
        assert!(layout.records_dir.is_dir());
        assert_eq!(fs::read_dir(&layout.records_dir).unwrap().count(), 0);
        let mut synced = Vec::new();
        layout
            .prepare_with_sync(|path| {
                synced.push(path.to_path_buf());
                sync_directory(path)
            })
            .unwrap();
        assert_eq!(
            &synced[..2],
            &[layout.records_dir.clone(), layout.temporary_dir.clone()]
        );
        assert_eq!(
            &synced[2..],
            &layout
                .workspace_dir
                .ancestors()
                .map(Path::to_path_buf)
                .collect::<Vec<_>>()
        );
        assert!(
            sync_directory(&sandbox.0.join("absent"))
                .unwrap_err()
                .contains("sync directory")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn inherited_acls_reject_store_and_export_without_writing_data() {
        let sandbox = Sandbox::new();
        let parent = sandbox.0.join("parent");
        fs::create_dir(&parent).unwrap();
        assert!(
            std::process::Command::new("/bin/chmod")
                .args([
                    "+a",
                    "everyone allow read,list,search,file_inherit,directory_inherit"
                ])
                .arg(&parent)
                .status()
                .unwrap()
                .success()
        );
        let layout = Layout::for_workspace(&parent.join("store"), &sandbox.0).unwrap();
        assert!(
            layout
                .prepare_for_write()
                .unwrap_err()
                .contains("extended ACL")
        );
        assert!(!layout.records_dir.exists());
        let output = parent.join("export.html");
        assert!(
            util::write_private_new(&output, b"private bytes")
                .unwrap_err()
                .contains("extended ACL")
        );
        assert!(!output.exists());
        // Retrying the now-existing managed directory still fails;
        // neither its inherited ACL nor the parent ACL is silently stripped.
        assert!(
            layout
                .prepare_for_write()
                .unwrap_err()
                .contains("extended ACL")
        );
        assert!(
            util::check_private_file(&File::open(&parent).unwrap())
                .unwrap_err()
                .contains("extended ACL")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn restrictive_acls_are_preserved_and_later_grants_are_rejected() {
        let sandbox = Sandbox::new();
        let parent = sandbox.0.join("parent");
        fs::create_dir(&parent).unwrap();
        assert!(
            std::process::Command::new("/bin/chmod")
                .args([
                    "+a",
                    "everyone deny writeextattr,file_inherit,directory_inherit"
                ])
                .arg(&parent)
                .status()
                .unwrap()
                .success()
        );
        let layout = Layout::for_workspace(&parent.join("store"), &sandbox.0).unwrap();
        layout.prepare_for_write().unwrap();
        let output = parent.join("output");
        util::write_private_new(&output, b"private bytes").unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"private bytes");
        let acl = std::process::Command::new("/bin/ls")
            .args(["-led"])
            .arg(&output)
            .output()
            .unwrap();
        assert!(acl.status.success());
        assert!(String::from_utf8_lossy(&acl.stdout).contains("inherited deny writeextattr"));
        // A safe first entry must not hide a permission-granting later entry.
        assert!(
            std::process::Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(&output)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            util::check_private_file(&File::open(&output).unwrap())
                .unwrap_err()
                .contains("permission-granting")
        );
        assert_eq!(fs::read(&output).unwrap(), b"private bytes");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn store_ancestors_are_checked_before_side_effects() {
        use std::os::unix::fs::symlink;
        let sandbox = Sandbox::new();
        let real = sandbox.0.join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(real.join("sentinel"), b"unchanged").unwrap();
        let alias = sandbox.0.join("alias");
        symlink(&real, &alias).unwrap();
        let dangling = sandbox.0.join("dangling");
        symlink(sandbox.0.join("absent"), &dangling).unwrap();
        for root in [
            alias.clone(),
            alias.join("new-store"),
            alias.join("..").join("cancelled-store"),
            dangling.join("new-store"),
        ] {
            let layout = Layout::for_workspace(&root, &sandbox.0).unwrap();
            assert!(
                layout
                    .prepare_for_write()
                    .unwrap_err()
                    .contains("symlinked")
            );
            assert!(
                layout
                    .records_available()
                    .unwrap_err()
                    .contains("symlinked")
            );
        }
        assert_eq!(fs::read(real.join("sentinel")).unwrap(), b"unchanged");
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(fs::read_dir(&real).unwrap().count(), 1);
        assert!(!sandbox.0.join("cancelled-store").exists());
        assert!(!sandbox.0.join("absent").exists());

        let root = sandbox.0.join("ordinary-store");
        let layout = Layout::for_workspace(&root, &sandbox.0).unwrap();
        assert!(!layout.records_available().unwrap());
        layout.prepare_for_write().unwrap();
        assert!(layout.records_available().unwrap());
        // A link in a later destination must fail before chmod of earlier ones.
        fs::remove_dir(&layout.temporary_dir).unwrap();
        symlink(&real, &layout.temporary_dir).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            layout
                .prepare_for_write()
                .unwrap_err()
                .contains("symlinked")
        );
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(fs::read(real.join("sentinel")).unwrap(), b"unchanged");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn missing_prefix_parent_traversal_is_rejected_before_creation() {
        use std::os::unix::fs::symlink;
        let sandbox = Sandbox::new();
        let real = sandbox.0.join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(real.join("sentinel"), b"keep").unwrap();
        symlink(&real, sandbox.0.join("alias")).unwrap();
        for suffix in [
            "missing/../alias",
            "missing/../alias/new-store",
            "missing/child/../../alias",
        ] {
            let layout = Layout::for_workspace(&sandbox.0.join(suffix), &sandbox.0).unwrap();
            for error in [
                layout.records_available().unwrap_err(),
                layout.prepare_for_write().unwrap_err(),
            ] {
                assert!(error.contains("parent traversal after missing"), "{error}");
            }
            assert!(!sandbox.0.join("missing").exists());
            assert_eq!(
                fs::metadata(&real).unwrap().permissions().mode() & 0o777,
                0o755
            );
            assert_eq!(fs::read_dir(&real).unwrap().count(), 1);
            assert_eq!(fs::read(real.join("sentinel")).unwrap(), b"keep");
        }
        let nested =
            Layout::for_workspace(&sandbox.0.join("ordinary/nested/store"), &sandbox.0).unwrap();
        assert!(!nested.records_available().unwrap());
        nested.prepare_for_write().unwrap();
        assert!(nested.records_available().unwrap());
        // Parent traversal through an existing physical directory remains valid.
        let existing_parent =
            Layout::for_workspace(&sandbox.0.join("ordinary/../another/store"), &sandbox.0)
                .unwrap();
        existing_parent.prepare_for_write().unwrap();
        assert!(existing_parent.records_available().unwrap());
    }

    #[test]
    fn publication_does_not_replace_existing_record() {
        let sandbox = Sandbox::new();
        let staged = sandbox.0.join("staged");
        let destination = sandbox.0.join("record");
        fs::write(&staged, b"new").unwrap();
        fs::write(&destination, b"original").unwrap();
        assert!(publish_no_clobber(&staged, &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert_eq!(fs::read(&staged).unwrap(), b"new");
    }

    #[test]
    fn publication_fails_closed_when_hard_link_is_unavailable() {
        let sandbox = Sandbox::new();
        // Directories cannot be hard-linked, but the removed rename fallback
        // would successfully move this directory to the absent destination.
        let staged = sandbox.0.join("staged");
        let destination = sandbox.0.join("record");
        fs::create_dir(&staged).unwrap();
        let error = publish_no_clobber(&staged, &destination).unwrap_err();
        assert!(
            error.contains("filesystem that permits hard links"),
            "{error}"
        );
        assert!(staged.is_dir());
        assert!(!destination.exists());
    }

    #[test]
    fn ordinary_utf8_identity_keeps_legacy_namespace() {
        let path = Path::new("ordinary/workspace");
        let expected = util::hex(&Sha256::digest(path.to_string_lossy().as_bytes())[..8]);
        assert_eq!(workspace_identity(path), (expected, None));
    }

    #[cfg(unix)]
    #[test]
    fn native_identity_separates_lossy_aliases_and_replacement_text() {
        use std::os::unix::ffi::OsStringExt;
        let paths = [
            PathBuf::from(std::ffi::OsString::from_vec(b"/workspace-\xff".to_vec())),
            PathBuf::from(std::ffi::OsString::from_vec(b"/workspace-\xfe".to_vec())),
            PathBuf::from("/workspace-\u{fffd}"),
        ];
        let identities = paths.map(|path| workspace_identity(&path));
        for (index, (current, legacy)) in identities.iter().enumerate() {
            assert!(current.starts_with("native-v1-"));
            assert_eq!(legacy, &identities[0].1);
            for other in &identities[index + 1..] {
                assert_ne!(current, &other.0);
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_identity_separates_unpaired_surrogates() {
        use std::os::windows::ffi::OsStringExt;
        let a = PathBuf::from(std::ffi::OsString::from_wide(&[0xd800]));
        let b = PathBuf::from(std::ffi::OsString::from_wide(&[0xd801]));
        let a = workspace_identity(&a);
        let b = workspace_identity(&b);
        assert_ne!(a.0, b.0);
        assert_eq!(a.1, b.1);
    }

    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    #[test]
    fn ambiguous_legacy_records_block_reads_and_writes_even_with_new_store() {
        let sandbox = Sandbox::new();
        let workspace = Path::new("workspace-\u{fffd}");
        let layout = Layout::for_workspace(&sandbox.0, workspace).unwrap();
        layout.prepare_for_write().unwrap();
        let legacy_records = layout.legacy_dir.as_ref().unwrap().join("records");
        fs::create_dir_all(&legacy_records).unwrap();
        // Empty legacy stores do not block. Corrupt records still require attribution.
        assert!(layout.records_available().unwrap());
        fs::write(legacy_records.join("old.piradec"), b"legacy").unwrap();
        for error in [
            Layout::for_workspace(&sandbox.0, workspace).unwrap_err(),
            layout.records_available().unwrap_err(),
            layout.prepare_for_write().unwrap_err(),
        ] {
            assert!(error.contains("ambiguous legacy"), "{error}");
            assert!(error.contains("manually attribute"), "{error}");
        }
        assert_eq!(
            fs::read(legacy_records.join("old.piradec")).unwrap(),
            b"legacy"
        );
        assert_eq!(fs::read_dir(&layout.records_dir).unwrap().count(), 0);
    }
}
