//! Tool-managed exact-caller worker defaults, isolated by the Team store.
use crate::storage::managed::{
    create_root, inspect_managed_file, load_config, lock_config, persist_config, root_if_present,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "worker_defaults.json";

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
        crate::harness::Harness::for_model(profile["model"].as_str().unwrap())
            .validate_effort(profile["effort"].as_str().unwrap())?;
    }
    Ok(())
}

/// Load and validate all overrides; an absent store/file means no overrides.
pub fn load(root: &Path) -> Result<Value, String> {
    load_config(root, FILE_NAME, "worker defaults", validate)
}

fn lock(root: &Path) -> Result<crate::native::FileLock, String> {
    lock_config(root, "worker_defaults.lock", "worker defaults")
}

fn persist(root: &Path, value: &Value) -> Result<(), String> {
    persist_config(root, FILE_NAME, "worker defaults", value)
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

/// Preserve the complete safe file before explicitly resetting all overrides.
fn reset_all(root: &Path) -> Result<Option<PathBuf>, String> {
    let Some(root) = root_if_present(root)? else {
        return Ok(None);
    };
    let _lock = lock(&root)?;
    let path = root.join(FILE_NAME);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => inspect_managed_file(&metadata, "worker defaults")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("inspect worker defaults: {e}")),
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| format!("defaults backup name: {e}"))?;
    let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let backup = root.join(format!("worker_defaults.{name}.backup"));
    fs::hard_link(&path, &backup)
        .map_err(|e| format!("preserve worker defaults backup: {e}; prior file was not changed"))?;
    inspect_managed_file(
        &fs::symlink_metadata(&backup).map_err(|e| e.to_string())?,
        "worker defaults backup",
    )?;
    persist(&root, &json!({}))?;
    Ok(Some(backup))
}

/// Execute config show/set/reset without requiring a Codex session or launching workers.
pub fn command(
    args: &[String],
    default_root: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<(), String> {
    if args.get(1).is_some_and(|arg| arg == "retention") {
        return crate::retention::policy::command(args, default_root);
    }
    let action = args
        .get(1)
        .map(String::as_str)
        .ok_or("expected config show|set|reset")?;
    if !["show", "set", "reset"].contains(&action) {
        return Err("expected config show|set|reset".into());
    }
    let (mut store, mut main, mut model, mut effort) = (None, None, None, None);
    let mut all = false;
    let mut rest = args[2..].iter();
    while let Some(option) = rest.next() {
        if option == "--all" && action == "reset" {
            if all {
                return Err("provide --all only once".into());
            }
            all = true;
            continue;
        }
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
    if all && main.is_some() {
        return Err("config reset --all and --main are mutually exclusive".into());
    }
    if action != "show" && main.is_none() && !all {
        return Err("config set/reset requires --main MODEL (or reset --all)".into());
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
    let mut backup = None;
    let overrides = match action {
        "show" => load(&root)?,
        "set" => set(
            &root,
            main.as_deref().unwrap(),
            model.as_deref().unwrap(),
            effort.as_deref().unwrap(),
        )?,
        "reset" if all => {
            backup = reset_all(&root)?;
            json!({})
        }
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
    let mut result = json!({"path": path, "bundled": bundled, "overrides": overrides});
    if all {
        result["backup"] = json!(backup);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/defaults.rs"]
mod tests;
