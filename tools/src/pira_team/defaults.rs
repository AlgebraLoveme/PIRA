//! Tool-managed exact-caller worker defaults, isolated by the Team store.
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "worker_defaults.json";
const MAX_BYTES: u64 = 64 * 1024;

fn validate(value: &Value) -> Result<(), String> {
    let entries = value
        .as_object()
        .ok_or("expected an exact-main-model object")?;
    for (main, profile) in entries {
        super::validate_model(main).map_err(|_| "invalid main model identifier")?;
        let fields = profile.as_object().ok_or("expected model/effort object")?;
        if fields.len() != 2 {
            return Err("each override must contain only model and effort".into());
        }
        super::validate_model(
            profile["model"]
                .as_str()
                .ok_or("missing/string model required")?,
        )?;
        super::validate_effort(
            profile["effort"]
                .as_str()
                .ok_or("missing/string effort required")?,
        )?;
    }
    Ok(())
}

fn inspect_file(metadata: &fs::Metadata) -> Result<(), String> {
    inspect_managed_file(metadata, "worker defaults")
}

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

fn create_root(root: &Path) -> Result<PathBuf, String> {
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

/// Load and validate all overrides; an absent store/file means no overrides.
pub fn load(root: &Path) -> Result<Value, String> {
    let Some(root) = root_if_present(root)? else {
        return Ok(json!({}));
    };
    let path = root.join(FILE_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(format!("inspect {}: {e}", path.display())),
    };
    inspect_file(&metadata)?;
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
    inspect_file(&file.metadata().map_err(|e| e.to_string())?)?;
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
            "invalid worker defaults {}: {e}; prior file was not changed",
            path.display()
        )
    })
}

fn lock(root: &Path) -> Result<File, String> {
    let path = root.join("worker_defaults.lock");
    match fs::symlink_metadata(&path) {
        Ok(metadata) => inspect_file(&metadata)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("inspect defaults lock: {e}")),
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
        .map_err(|e| format!("open defaults lock: {e}"))?;
    inspect_file(&file.metadata().map_err(|e| e.to_string())?)?;
    // OS-owned lock is released on process death; never delete this stable lock file.
    file.try_lock()
        .map_err(|e| format!("worker defaults are busy/unavailable: {e}; retry"))?;
    Ok(file)
}

fn persist(root: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_BYTES {
        return Err("worker defaults would exceed 64 KiB; prior file was not changed".into());
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| format!("defaults temporary name: {e}"))?;
    let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = root.join(format!(".worker_defaults-{name}.tmp"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|e| format!("create defaults temporary file: {e}"))?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|e| format!("write defaults: {e}"))?;
        file.sync_all().map_err(|e| format!("sync defaults: {e}"))?;
        drop(file);
        fs::rename(&temporary, root.join(FILE_NAME))
            .map_err(|e| format!("replace defaults: {e}"))?;
        #[cfg(unix)]
        File::open(root)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| format!("defaults replaced but directory sync failed: {e}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Set a complete override without losing concurrent updates or replacing invalid config.
pub fn set(root: &Path, main: &str, model: &str, effort: &str) -> Result<Value, String> {
    let entry = json!({main: {"model": model, "effort": effort}});
    validate(&entry)?;
    let root = create_root(root)?;
    let _lock = lock(&root)?;
    let mut value = load(&root)?;
    value[main] = entry[main].clone();
    persist(&root, &value)?;
    Ok(value)
}

/// Remove only one exact identifier; missing overrides are an idempotent no-op.
pub fn reset(root: &Path, main: &str) -> Result<Value, String> {
    super::validate_model(main).map_err(|_| "invalid main model identifier")?;
    let Some(root) = root_if_present(root)? else {
        return Ok(json!({}));
    };
    let _lock = lock(&root)?;
    let mut value = load(&root)?;
    if value.as_object_mut().unwrap().remove(main).is_some() {
        persist(&root, &value)?;
    }
    Ok(value)
}

/// Execute config show/set/reset without requiring a Codex session or launching workers.
pub fn command(
    args: &[String],
    default_root: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<(), String> {
    let action = args
        .get(1)
        .map(String::as_str)
        .ok_or("expected config show|set|reset")?;
    if !["show", "set", "reset"].contains(&action) {
        return Err("expected config show|set|reset".into());
    }
    let (mut store, mut main, mut model, mut effort) = (None, None, None, None);
    let mut rest = args[2..].iter();
    while let Some(option) = rest.next() {
        let slot = match option.as_str() {
            "--store" => &mut store,
            "--main" if action != "show" => &mut main,
            "--model" if action == "set" => &mut model,
            "--effort" if action == "set" => &mut effort,
            _ => return Err(format!("unknown config {action} option {option}")),
        };
        if slot.is_some() {
            return Err(format!("provide {option} only once"));
        }
        *slot = Some(
            rest.next()
                .ok_or_else(|| format!("missing value for {option}"))?
                .clone(),
        );
    }
    // Validate command shape before even selecting/creating a store.
    if action != "show" && main.is_none() {
        return Err("config set/reset requires --main MODEL".into());
    }
    if action == "set" && (model.is_none() || effort.is_none()) {
        return Err("config set requires --model WORKER and --effort EFFORT".into());
    }
    if store.as_deref() == Some("") {
        return Err("--store must not be empty".into());
    }
    let root = store
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(default_root)?;
    let overrides = match action {
        "show" => load(&root)?,
        "set" => set(
            &root,
            main.as_deref().unwrap(),
            model.as_deref().unwrap(),
            effort.as_deref().unwrap(),
        )?,
        "reset" => reset(&root, main.as_deref().unwrap())?,
        _ => unreachable!(),
    };
    let path = if root.is_absolute() {
        root
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(root)
    }
    .join(FILE_NAME);
    let bundled: Value = serde_json::from_str(super::WORKER_PROFILES)
        .map_err(|e| format!("invalid bundled worker profiles: {e}"))?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"path": path, "bundled": bundled, "overrides": overrides})
        )
        .map_err(|e| e.to_string())?
    );
    Ok(())
}

#[cfg(test)]
#[path = "defaults_tests.rs"]
mod tests;
