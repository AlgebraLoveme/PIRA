//! Store-scoped retention settings; shares managed config I/O with worker defaults.
use crate::storage::managed;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) const DAY_MS: u64 = 86_400_000;
pub(super) const GB: u64 = 1_000_000_000;
pub(super) const DEFAULT_MAX_AGE_DAYS: u64 = 30;
pub(super) const DEFAULT_TARGET_GB: u64 = 50;
const MAX_AGE_DAYS: u64 = 36_500;
const MAX_TARGET_GB: u64 = 1_000_000;
const FILE_NAME: &str = "retention_policy.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Policy {
    max_age_days: u64,
    target_gb: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_age_days: DEFAULT_MAX_AGE_DAYS,
            target_gb: DEFAULT_TARGET_GB,
        }
    }
}

fn threshold(value: u64, maximum: u64, name: &str) -> Result<u64, String> {
    if (1..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(format!("{name} must be an integer in 1..={maximum}"))
    }
}

impl Policy {
    fn parse(value: &Value) -> Result<Self, String> {
        let fields = value
            .as_object()
            .ok_or("expected retention policy object")?;
        if fields
            .keys()
            .any(|key| !["max_age_days", "target_gb"].contains(&key.as_str()))
        {
            return Err("retention policy accepts only max_age_days and target_gb".into());
        }
        let field = |name, default, maximum| match fields.get(name) {
            None => Ok(default),
            Some(value) => threshold(
                value
                    .as_u64()
                    .ok_or_else(|| format!("{name} must be a positive integer"))?,
                maximum,
                name,
            ),
        };
        Ok(Self {
            max_age_days: field("max_age_days", DEFAULT_MAX_AGE_DAYS, MAX_AGE_DAYS)?,
            target_gb: field("target_gb", DEFAULT_TARGET_GB, MAX_TARGET_GB)?,
        })
    }
    pub(super) fn max_age_ms(self) -> u64 {
        self.max_age_days * DAY_MS
    }
    pub(super) fn target_bytes(self) -> u64 {
        self.target_gb * GB
    }
    pub(super) fn value(self) -> Value {
        json!({"max_age_days":self.max_age_days,"target_gb":self.target_gb,
            "max_age_ms":self.max_age_ms(),"target_bytes":self.target_bytes()})
    }
    fn settings(self) -> Value {
        json!({"max_age_days":self.max_age_days,"target_gb":self.target_gb})
    }
}

fn overrides(root: &Path) -> Result<Value, String> {
    managed::load_config(root, FILE_NAME, "retention policy", |value| {
        Policy::parse(value).map(|_| ())
    })
}

pub(super) fn load(root: &Path) -> Result<Policy, String> {
    Policy::parse(&overrides(root)?)
}

fn set(root: &Path, age: Option<u64>, target: Option<u64>) -> Result<Policy, String> {
    let root = managed::create_root(root)?;
    let _lock = managed::lock_config(&root, "retention_policy.lock", "retention policy")?;
    let mut policy = load(&root)?;
    if let Some(age) = age {
        policy.max_age_days = threshold(age, MAX_AGE_DAYS, "max_age_days")?;
    }
    if let Some(target) = target {
        policy.target_gb = threshold(target, MAX_TARGET_GB, "target_gb")?;
    }
    managed::persist_config(&root, FILE_NAME, "retention policy", &policy.settings())?;
    Ok(policy)
}

fn reset(root: &Path) -> Result<Policy, String> {
    let Some(root) = managed::root_if_present(root)? else {
        return Ok(Policy::default());
    };
    let _lock = managed::lock_config(&root, "retention_policy.lock", "retention policy")?;
    match std::fs::symlink_metadata(root.join(FILE_NAME)) {
        Ok(metadata) => managed::inspect_managed_file(&metadata, "retention policy")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Policy::default()),
        Err(error) => return Err(format!("inspect retention policy: {error}")),
    }
    // Explicit reset can recover malformed contents, never a symlink/unsafe file.
    managed::persist_config(&root, FILE_NAME, "retention policy", &json!({}))?;
    Ok(Policy::default())
}

fn number(value: Option<String>, maximum: u64, name: &str) -> Result<Option<u64>, String> {
    value
        .map(|value| {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(format!("{name} must be an integer in 1..={maximum}"));
            }
            threshold(
                value.parse().map_err(|_| format!("{name} is too large"))?,
                maximum,
                name,
            )
        })
        .transpose()
}

pub(crate) fn command(
    args: &[String],
    default_root: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<(), String> {
    let action = args
        .get(2)
        .map(String::as_str)
        .ok_or("expected config retention show|set|reset")?;
    if !["show", "set", "reset"].contains(&action) {
        return Err("expected config retention show|set|reset".into());
    }
    let (mut store, mut age, mut target) = (None, None, None);
    let mut rest = args[3..].iter();
    while let Some(option) = rest.next() {
        let slot = match option.as_str() {
            "--store" => &mut store,
            "--max-age-days" if action == "set" => &mut age,
            "--target-gb" if action == "set" => &mut target,
            _ => return Err(format!("unknown config retention {action} option {option}")),
        };
        crate::set_once(
            slot,
            rest.next()
                .ok_or_else(|| format!("missing value for {option}"))?
                .clone(),
            option,
        )?;
    }
    if action == "set" && age.is_none() && target.is_none() {
        return Err("config retention set requires --max-age-days and/or --target-gb".into());
    }
    let age = number(age, MAX_AGE_DAYS, "max_age_days")?;
    let target = number(target, MAX_TARGET_GB, "target_gb")?;
    if store.as_deref() == Some("") {
        return Err("--store must not be empty".into());
    }
    let root = store
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(default_root)?;
    let policy = match action {
        "show" => load(&root)?,
        "set" => set(&root, age, target)?,
        "reset" => reset(&root)?,
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
    println!(
        "{}",
        json!({"path":path,"bundled":Policy::default().value(),"effective":policy.value()})
    );
    Ok(())
}
