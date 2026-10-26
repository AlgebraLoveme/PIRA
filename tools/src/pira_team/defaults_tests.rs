use super::super::{resolve, resolve_with_store};
use super::*;

struct Store(PathBuf);
impl Store {
    fn new() -> Self {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        Self(std::env::temp_dir().join(format!(
            "team-defaults-{}-{:x}",
            std::process::id(),
            u64::from_le_bytes(random)
        )))
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn caller(model: &str) -> Value {
    json!({"model":model, "effort":"ultra", "approval_policy":"never",
        "sandbox_policy":{"type":"danger-full-access"}})
}

#[test]
fn absent_config_is_read_only_and_preserves_all_bundled_categories() {
    let store = Store::new();
    assert_eq!(load(&store.0).unwrap(), json!({}));
    assert_eq!(reset(&store.0, "missing").unwrap(), json!({}));
    for model in [
        "gpt-6-astra",
        "gpt-6.1-sol",
        "gpt-6-sol",
        "gpt-6-luna",
        "gpt-5.6-sol",
        "unknown",
    ] {
        let baseline = resolve(None, None, caller(model)).unwrap();
        let actual = resolve_with_store(None, None, caller(model), &store.0).unwrap();
        assert_eq!(
            (actual.0, actual.1, actual.2),
            (baseline.0, baseline.1, baseline.2)
        );
        assert_eq!(actual.3.sandbox, baseline.3.sandbox);
    }
    assert!(!store.0.exists());
}

#[test]
fn exact_overrides_cover_mapping_inheritance_and_unknown_callers() {
    let store = Store::new();
    for main in ["gpt-6-astra", "gpt-6-sol", "unknown"] {
        set(&store.0, main, "custom-worker", "low").unwrap();
        let (model, effort, sources, _) =
            resolve_with_store(None, None, caller(main), &store.0).unwrap();
        assert_eq!((model.as_str(), effort.as_str()), ("custom-worker", "low"));
        assert_eq!(sources, json!({"model":"config", "effort":"config"}));
    }
    for main in ["GPT-6-ASTRA", "gpt-6-astra-preview", "unknown-preview"] {
        let (model, effort, sources, _) =
            resolve_with_store(None, None, caller(main), &store.0).unwrap();
        assert_eq!((model.as_str(), effort.as_str()), ("gpt-6.1-sol", "high"));
        assert_eq!(sources, json!({"model":"fallback", "effort":"fallback"}));
    }
}

#[test]
fn explicit_fields_win_independently_and_do_not_change_caller_lookup() {
    let store = Store::new();
    set(&store.0, "gpt-6-astra", "custom", "low").unwrap();
    for (model, effort, expected, sources) in [
        (
            Some("explicit-worker"),
            None,
            ("explicit-worker", "low"),
            json!({"model":"explicit", "effort":"config"}),
        ),
        (
            None,
            Some("medium"),
            ("custom", "medium"),
            json!({"model":"config", "effort":"explicit"}),
        ),
        (
            Some("explicit-worker"),
            Some("medium"),
            ("explicit-worker", "medium"),
            json!({"model":"explicit", "effort":"explicit"}),
        ),
    ] {
        let (model, effort, actual_sources, _) = resolve_with_store(
            model.map(str::to_owned),
            effort.map(str::to_owned),
            caller("gpt-6-astra"),
            &store.0,
        )
        .unwrap();
        assert_eq!((model.as_str(), effort.as_str()), expected);
        assert_eq!(actual_sources, sources);
    }
    let mut context = caller("gpt-6-astra");
    context["approval_policy"] = json!("on-request");
    assert!(
        resolve_with_store(None, None, context, &store.0)
            .unwrap_err()
            .contains("approval")
    );
}

#[test]
fn persistence_reset_and_store_isolation() {
    let one = Store::new();
    let two = Store::new();
    set(&one.0, "gpt-6-astra", "first", "low").unwrap();
    set(&one.0, "gpt-6-sol", "second", "high").unwrap();
    assert_eq!(load(&two.0).unwrap(), json!({}));
    assert_eq!(
        load(&one.0).unwrap(),
        json!({"gpt-6-astra":{"model":"first","effort":"low"},"gpt-6-sol":{"model":"second","effort":"high"}})
    );
    reset(&one.0, "gpt-6-astra").unwrap();
    assert_eq!(
        load(&one.0).unwrap(),
        json!({"gpt-6-sol":{"model":"second","effort":"high"}})
    );
    let before = fs::read(one.0.join(FILE_NAME)).unwrap();
    reset(&one.0, "not-configured").unwrap();
    assert_eq!(fs::read(one.0.join(FILE_NAME)).unwrap(), before);
    reset(&one.0, "gpt-6-sol").unwrap();
    assert_eq!(load(&one.0).unwrap(), json!({}));
    assert!(
        fs::read_dir(&one.0).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp"))
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(one.0.join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn invalid_fields_never_replace_prior_valid_config() {
    let store = Store::new();
    set(&store.0, "main", "worker", "high").unwrap();
    let before = fs::read(store.0.join(FILE_NAME)).unwrap();
    for main in ["", "-main", "bad main", "é"] {
        assert!(set(&store.0, main, "worker", "high").is_err());
    }
    assert!(set(&store.0, "main", &"x".repeat(257), "high").is_err());
    assert!(set(&store.0, "main", "worker", "invalid").is_err());
    assert!(reset(&store.0, "bad main").is_err());
    assert_eq!(fs::read(store.0.join(FILE_NAME)).unwrap(), before);
    set(&store.0, &"m".repeat(256), &"w".repeat(256), "none").unwrap();
}

#[test]
fn malformed_or_oversized_config_fails_all_new_reads_and_edits_without_replacement() {
    let store = Store::new();
    create_root(&store.0).unwrap();
    for bytes in [
        b"{".to_vec(),
        b"[]".to_vec(),
        br#"{"main":{"model":"worker"}}"#.to_vec(),
        br#"{"main":{"model":"worker","effort":"high","extra":true}}"#.to_vec(),
        br#"{"main":{"model":1,"effort":"high"}}"#.to_vec(),
        br#"{"main":{"model":"worker","effort":"invalid"}}"#.to_vec(),
        vec![b' '; MAX_BYTES as usize + 1],
    ] {
        fs::write(store.0.join(FILE_NAME), &bytes).unwrap();
        assert!(load(&store.0).is_err());
        assert!(set(&store.0, "other", "worker", "low").is_err());
        assert!(reset(&store.0, "main").is_err());
        assert!(
            resolve_with_store(
                Some("explicit".into()),
                Some("high".into()),
                caller("main"),
                &store.0
            )
            .is_err()
        );
        assert_eq!(fs::read(store.0.join(FILE_NAME)).unwrap(), bytes);
        // Lifecycle's retained-field resolver deliberately never reads current config.
        let retained = resolve(
            Some("retained".into()),
            Some("medium".into()),
            caller("main"),
        )
        .unwrap();
        assert_eq!(
            (retained.0.as_str(), retained.1.as_str()),
            ("retained", "medium")
        );
    }
}

#[test]
fn lock_contention_is_visible_and_release_allows_update() {
    let store = Store::new();
    set(&store.0, "first", "worker", "high").unwrap();
    let held = lock(&store.0).unwrap();
    assert!(
        set(&store.0, "second", "worker", "low")
            .unwrap_err()
            .contains("busy")
    );
    assert!(load(&store.0).unwrap().get("second").is_none());
    drop(held);
    set(&store.0, "second", "worker", "low").unwrap();
    assert_eq!(load(&store.0).unwrap().as_object().unwrap().len(), 2);
}

#[test]
fn failed_atomic_replace_cleans_temporary_output() {
    let store = Store::new();
    create_root(&store.0).unwrap();
    fs::create_dir(store.0.join(FILE_NAME)).unwrap();
    assert!(
        persist(&store.0, &json!({}))
            .unwrap_err()
            .contains("replace")
    );
    assert!(store.0.join(FILE_NAME).is_dir());
    assert_eq!(fs::read_dir(&store.0).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn symlink_store_config_and_lock_are_rejected() {
    let store = Store::new();
    let alias = Store::new();
    set(&store.0, "main", "worker", "high").unwrap();
    std::os::unix::fs::symlink(&store.0, &alias.0).unwrap();
    assert!(load(&alias.0).is_err());
    fs::remove_file(&alias.0).unwrap();
    let target = store.0.join("target.json");
    fs::rename(store.0.join(FILE_NAME), &target).unwrap();
    std::os::unix::fs::symlink(&target, store.0.join(FILE_NAME)).unwrap();
    assert!(set(&store.0, "main", "other", "low").is_err());
    fs::remove_file(store.0.join(FILE_NAME)).unwrap();
    fs::rename(&target, store.0.join(FILE_NAME)).unwrap();
    fs::remove_file(store.0.join("worker_defaults.lock")).unwrap();
    std::os::unix::fs::symlink(
        store.0.join(FILE_NAME),
        store.0.join("worker_defaults.lock"),
    )
    .unwrap();
    assert!(reset(&store.0, "main").is_err());
    assert_eq!(load(&store.0).unwrap()["main"]["model"], "worker");
}
