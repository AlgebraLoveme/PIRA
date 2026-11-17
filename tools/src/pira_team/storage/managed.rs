//! Shared Team-store validation and bounded, locked, atomic configuration I/O.
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 64 * 1024;

pub(crate) fn inspect_managed_file(metadata: &fs::Metadata, subject: &str) -> Result<(), String> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{subject} paths must be regular files, not symlinks"
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(format!(
                "{subject} files must be user-owned and not group/other writable"
            ));
        }
    }
    Ok(())
}

pub(crate) fn root_if_present(root: &Path) -> Result<Option<PathBuf>, String> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("inspect Team store: {e}")),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Team store must be a real directory, not a symlink".into());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("resolve Team store: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
            return Err("Team store must be user-owned and not group/other writable".into());
        }
        for ancestor in root.ancestors() {
            let metadata =
                fs::metadata(ancestor).map_err(|e| format!("inspect store ancestor: {e}"))?;
            if (metadata.uid() != uid && metadata.uid() != 0)
                || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
            {
                return Err(format!(
                    "unsafe store ancestor {}; choose a private --store",
                    ancestor.display()
                ));
            }
        }
    }
    Ok(Some(root))
}

pub(crate) fn create_root(root: &Path) -> Result<PathBuf, String> {
    if let Some(root) = root_if_present(root)? {
        return Ok(root);
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(root)
        .map_err(|e| format!("create Team store: {e}"))?;
    root_if_present(root)?.ok_or_else(|| "Team store disappeared".into())
}

pub(crate) fn load_config(
    root: &Path,
    file_name: &str,
    subject: &str,
    validate: impl FnOnce(&Value) -> Result<(), String>,
) -> Result<Value, String> {
    let Some(root) = root_if_present(root)? else {
        return Ok(json!({}));
    };
    let path = root.join(file_name);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(format!("inspect {}: {e}", path.display())),
    };
    inspect_managed_file(&metadata, subject)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    inspect_managed_file(&file.metadata().map_err(|e| e.to_string())?, subject)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{} exceeds 64 KiB", path.display()));
    }
    let result = serde_json::from_slice(&bytes)
        .map_err(|e| e.to_string())
        .and_then(|value| validate(&value).map(|()| value));
    result.map_err(|e| {
        format!(
            "invalid {subject} {}: {e}; prior file was not changed",
            path.display()
        )
    })
}

pub(crate) fn lock_config(
    root: &Path,
    file_name: &str,
    subject: &str,
) -> Result<crate::native::FileLock, String> {
    let path = root.join(file_name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => inspect_managed_file(&metadata, subject)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("inspect {subject} lock: {e}")),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("open {subject} lock: {e}"))?;
    inspect_managed_file(&file.metadata().map_err(|e| e.to_string())?, subject)?;
    // OS-owned lock is released on process death; never delete this stable lock file.
    file.try_lock()
        .map_err(|e| format!("{subject} are busy/unavailable: {e}; retry"))?;
    Ok(crate::native::FileLock(file))
}

pub(crate) fn persist_config(
    root: &Path,
    file_name: &str,
    subject: &str,
    value: &Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!(
            "{subject} would exceed 64 KiB; prior file was not changed"
        ));
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| format!("{subject} temporary name: {e}"))?;
    let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = root.join(format!(".{file_name}-{name}.tmp"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|e| format!("create {subject} temporary file: {e}"))?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|e| format!("write {subject}: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("sync {subject}: {e}"))?;
        drop(file);
        fs::rename(&temporary, root.join(file_name))
            .map_err(|e| format!("replace {subject}: {e}"))?;
        super::sync_directory(root)
            .map_err(|e| format!("{subject} replaced but directory sync failed: {e}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut bytes = [0; 8];
            getrandom::fill(&mut bytes).unwrap();
            let root =
                std::env::temp_dir().join(format!("team-managed-{:x}", u64::from_ne_bytes(bytes)));
            crate::private_dir(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn run_allocation_shares_root_checks_but_not_read_only_creation_semantics() {
        let fixture = Fixture::new();
        let root = fixture.0.join("absent");
        assert_eq!(root_if_present(&root).unwrap(), None);
        assert!(!root.exists());
        let first = crate::storage::allocate_run(&root).unwrap();
        let second = crate::storage::allocate_run(&root).unwrap();
        assert_ne!(first, second);
        assert_eq!(root_if_present(&root).unwrap().as_deref(), first.parent());
        crate::storage::validate_run_id(first.file_name().unwrap().to_str().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_roots_reject_run_allocation_and_configuration_without_new_children() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        let target = fixture.0.join("target");
        crate::private_dir(&target).unwrap();
        let alias = fixture.0.join("alias");
        symlink(&target, &alias).unwrap();
        let writable = fixture.0.join("writable");
        crate::private_dir(&writable).unwrap();
        fs::set_permissions(&writable, fs::Permissions::from_mode(0o770)).unwrap();
        let ancestor = fixture.0.join("unsafe-ancestor");
        crate::private_dir(&ancestor).unwrap();
        fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o777)).unwrap();
        let nested = ancestor.join("private-child");
        crate::private_dir(&nested).unwrap();
        for root in [alias, writable, nested] {
            let count = fs::read_dir(&root).unwrap().count();
            assert!(root_if_present(&root).is_err(), "{}", root.display());
            assert!(
                crate::storage::allocate_run(&root).is_err(),
                "{}",
                root.display()
            );
            assert_eq!(fs::read_dir(&root).unwrap().count(), count);
        }
    }

    #[test]
    fn config_write_errors_name_the_feature_and_preserve_uncommitted_state() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("blocked.json")).unwrap();
        let error =
            persist_config(&fixture.0, "blocked.json", "retention policy", &json!({})).unwrap_err();
        assert!(error.contains("replace retention policy"), "{error}");
        assert!(!error.contains("defaults"), "{error}");
        assert!(fixture.0.join("blocked.json").is_dir());
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
        persist_config(
            &fixture.0,
            "state.json",
            "retention policy",
            &json!({"keep":true}),
        )
        .unwrap();
        let prior = fs::read(fixture.0.join("state.json")).unwrap();
        let error = persist_config(
            &fixture.0,
            "state.json",
            "retention policy",
            &json!("x".repeat(MAX_BYTES as usize)),
        )
        .unwrap_err();
        assert!(
            error.contains("retention policy would exceed 64 KiB"),
            "{error}"
        );
        assert_eq!(fs::read(fixture.0.join("state.json")).unwrap(), prior);
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 2);
    }
}
