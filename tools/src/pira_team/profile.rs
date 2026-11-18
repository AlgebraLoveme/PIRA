//! Worker profile selection, verified permission policy and shared identity readers.
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub(crate) const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;
const WORKER_PROFILES: &str = include_str!("worker_profiles.json");

pub(crate) mod build_roots;
pub mod defaults;
pub(crate) mod service_tier;

/// Resolve bundled defaults only; resumes supply both retained fields here.
pub fn resolve(
    model: Option<String>,
    effort: Option<String>,
    parent: Value,
) -> Result<(String, String, Value, Execution), String> {
    resolve_using(model, effort, parent, &json!({}))
}

/// Resolve new-launch defaults from the selected Team store, then explicit fields.
pub fn resolve_with_store(
    model: Option<String>,
    effort: Option<String>,
    parent: Value,
    store: &Path,
) -> Result<(String, String, Value, Execution), String> {
    // Fully explicit profiles do not consume defaults (or silently repair them).
    let overrides = if model.is_some() && effort.is_some() {
        json!({})
    } else {
        defaults::load(store)?
    };
    resolve_using(model, effort, parent, &overrides)
}

fn resolve_using(
    model: Option<String>,
    effort: Option<String>,
    parent: Value,
    overrides: &Value,
) -> Result<(String, String, Value, Execution), String> {
    if let Some(m) = &model {
        validate_model(m)?;
    }
    if let Some(e) = &effort {
        validate_effort(e)?;
    }
    let execution = Execution::from_context(&parent)?;
    let profiles: Value = serde_json::from_str(WORKER_PROFILES)
        .map_err(|e| format!("invalid bundled worker profiles: {e}"))?;
    // Match the caller, not an explicit worker model; resumes supply both stored fields.
    let caller_model = parent["model"].as_str();
    let configured = caller_model.and_then(|model| overrides.get(model));
    let mapped = caller_model.and_then(|model| profiles["mappings"].get(model));
    let inherit = profiles["inherit"]
        .as_array()
        .ok_or("invalid bundled worker inheritance list")?;
    let (defaults, source) = match (configured, mapped) {
        (Some(profile), _) => (profile, "config"),
        (None, Some(profile)) => (profile, "mapping"),
        (None, None)
            if caller_model.is_some_and(|model| {
                crate::harness::Harness::for_model(model).inherits_native_profile()
                    || inherit.iter().any(|entry| entry.as_str() == Some(model))
            }) =>
        {
            (&parent, "parent")
        }
        (None, None) => (&profiles["fallback"], "fallback"),
    };
    let field = |name: &str, explicit: Option<String>| {
        if let Some(value) = explicit {
            return Ok((value, "explicit"));
        }
        defaults
            .get(name)
            .and_then(Value::as_str)
            .map(|s| (s.to_owned(), source))
            .ok_or_else(|| {
                if source == "parent" {
                    format!("parent {name} is unavailable; supply --{name} explicitly")
                } else {
                    format!("invalid bundled worker {source} {name}")
                }
            })
    };
    let (model, model_source) = field("model", model)?;
    let (effort, effort_source) = field("effort", effort)?;
    let sources = json!({"model": model_source, "effort": effort_source});
    validate_model(&model)?;
    validate_effort(&effort)?;
    crate::harness::Harness::for_model(&model).validate_effort(&effort)?;
    Ok((model, effort, sources, execution))
}

fn validate_model(model: &str) -> Result<(), String> {
    if model.is_empty()
        || model.len() > 256
        || model.starts_with('-')
        || !model.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err("invalid model identifier; supply --model explicitly".into());
    }
    Ok(())
}
fn validate_effort(effort: &str) -> Result<(), String> {
    if ![
        "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
    ]
    .contains(&effort)
    {
        return Err("unrecognized reasoning effort; supply --effort explicitly".into());
    }
    Ok(())
}

pub(crate) fn validate_session_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 200
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("invalid main session identity".into());
    }
    Ok(())
}

/// Verify caller identity using the matching session log, without requiring worker writes.
pub(crate) fn caller_session() -> Result<String, String> {
    let identity = if let Some(context) = crate::remote::delegated_context() {
        context["caller_thread_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "missing main session identity".to_owned())
    } else {
        crate::harness::Harness::caller_session()
    }
    .map_err(|e| format!("verified main session identity is unavailable: {e}"))?;
    validate_session_id(&identity)?;
    Ok(identity)
}

pub(crate) fn native_caller_identity(manifest: &Value) -> Option<String> {
    let session = manifest["thread_id"].as_str().filter(|id| !id.is_empty())?;
    // Missing transport is a legacy Codex manifest; unknown explicit transports
    // never acquire Codex identity by default.
    let harness = match manifest.get("transport") {
        None | Some(Value::Null) => crate::harness::Harness::Codex,
        Some(value) => crate::harness::Harness::from_transport(value.as_str()?)?,
    };
    Some(harness.caller_identity(session))
}

pub(crate) fn matches_native_session(manifest: &Value, context: &Value) -> bool {
    native_caller_identity(manifest)
        .as_deref()
        .zip(context["caller_thread_id"].as_str())
        .is_some_and(|(native, caller)| native == caller)
}

pub fn load() -> Result<Value, String> {
    if let Some(context) = crate::remote::delegated_context() {
        return Ok(context);
    }
    crate::harness::Harness::caller_context()
}

// Identity-only readers stop at the first complete identity-bearing record. Later
// transcript writes cannot revoke an already verified immutable session identity.
pub(crate) fn read_identity(
    path: &Path,
    identity: impl Fn(&Value) -> Option<&Value>,
) -> Result<String, String> {
    let file = File::open(path).map_err(|_| "cannot open caller session log")?;
    read_identity_file(file, identity)
}

pub(crate) fn read_identity_file(
    file: File,
    identity: impl Fn(&Value) -> Option<&Value>,
) -> Result<String, String> {
    let len = file
        .metadata()
        .map_err(|_| "cannot inspect caller session log")?
        .len();
    let mut reader = BufReader::new(file.take(len));
    loop {
        let mut line = Vec::new();
        let size = reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| "cannot read caller session log")?;
        if size == 0 || line.last() != Some(&b'\n') {
            return Err("caller session identity was not verified".into());
        }
        if size as u64 > MAX_RECORD_BYTES {
            return Err("caller record exceeds 16 MiB".into());
        }
        let record: Value =
            serde_json::from_slice(&line).map_err(|_| "malformed caller session record")?;
        if let Some(id) = identity(&record) {
            return id
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "malformed caller identity".into());
        }
    }
}

/// A verified caller policy, translated to the native app-server representation.
#[derive(Clone, Debug)]
pub struct Execution {
    pub mode: &'static str,
    pub sandbox: Value,
    pub caller_thread_id: Value,
}

impl Execution {
    pub fn from_context(context: &Value) -> Result<Self, String> {
        if context["approval_policy"] != "never" {
            return Err("cannot inherit caller approval policy: Team supports only verified approval-never; interactive/granular approvals are unsupported (do not disable caller approvals)".into());
        }
        let policy = &context["sandbox_policy"];
        let object = policy
            .as_object()
            .ok_or("missing/malformed caller sandbox_policy")?;
        let mode;
        let sandbox;
        match policy["type"].as_str() {
            Some("danger-full-access") if object.len() == 1 => {
                mode = "danger-full-access";
                sandbox = json!({"type":"dangerFullAccess"});
            }
            Some("workspace-write") => {
                if object.keys().any(|key| !["type", "writable_roots", "network_access", "exclude_tmpdir_env_var", "exclude_slash_tmp"].contains(&key.as_str())) {
                    return Err("unsupported caller workspace-write fields".into());
                }
                let boolean = |name: &str| policy[name].as_bool().ok_or_else(|| format!("missing/malformed caller {name}"));
                let cwd = physical(Path::new(context["cwd"].as_str().ok_or("missing caller cwd")?))?;
                let mut roots = vec![cwd];
                for root in policy["writable_roots"].as_array().ok_or("missing/malformed caller writable_roots")? {
                    let root = physical(Path::new(root.as_str().ok_or("malformed caller writable root")?))?;
                    if !roots.contains(&root) { roots.push(root); }
                }
                mode = "workspace-write";
                sandbox = json!({"type":"workspaceWrite", "writableRoots":roots,
                    "networkAccess":boolean("network_access")?,
                    "excludeTmpdirEnvVar":boolean("exclude_tmpdir_env_var")?,
                    "excludeSlashTmp":boolean("exclude_slash_tmp")?});
            }
            Some("read-only") => return Err("read-only caller is unsupported: Team requires managed scratch/handoff/tool-store writes; no workspace-write elevation was attempted".into()),
            _ => return Err("missing/unknown/unsupported caller sandbox policy".into()),
        }
        // New Codex permission profiles can override the legacy sandbox projection.
        // The current thread/turn API cannot encode arbitrary restricted filesystem rules.
        let profile = &context["permission_profile"];
        if !profile.is_null()
            && (mode != "danger-full-access"
                || (*profile != json!({"type":"disabled"})
                    && *profile
                        != json!({
                            "type":"managed", "file_system":{"type":"unrestricted"}, "network":"enabled"
                        })))
        {
            return Err("unsupported caller permission_profile: cannot faithfully represent fine-grained/unknown permissions with this native Team API".into());
        }
        let filesystem = &context["file_system_sandbox_policy"];
        if !filesystem.is_null()
            && (mode != "danger-full-access" || *filesystem != json!({"type":"unrestricted"}))
        {
            return Err("unsupported caller file_system_sandbox_policy; refusing to discard filesystem restrictions".into());
        }
        Ok(Self {
            mode,
            sandbox,
            caller_thread_id: context["caller_thread_id"].clone(),
        })
    }

    pub fn require_writable(&self, path: &Path) -> Result<(), String> {
        if self.mode == "danger-full-access" {
            return Ok(());
        }
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join(path)
        };
        let path = physical(&path)?;
        let allowed = self.sandbox["writableRoots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|root| {
                let root = Path::new(root.as_str().unwrap());
                path.strip_prefix(root).is_ok_and(|relative| {
                    !relative.components().any(|c| {
                        [".git", ".codex", ".agents"]
                            .iter()
                            .any(|name| c.as_os_str() == *name)
                    })
                })
            });
        if !allowed {
            return Err(format!(
                "caller permissions do not allow required Team write path {}; place Team/tool stores, cwd and build/cache roots within caller writable scope; no extra write root was granted",
                path.display()
            ));
        }
        Ok(())
    }

    // Check the caller's original implicit temp grants before replacing TMPDIR.
    // The worker keeps the same sandbox flags; no ambient directory is added as a root.
    pub(super) fn require_temp_writable(&self, path: &Path) -> Result<(), String> {
        if self.require_writable(path).is_ok() {
            return Ok(());
        }
        let path = physical(path)?;
        let mut roots = Vec::new();
        if self.sandbox["excludeTmpdirEnvVar"] == false
            && let Some(root) = std::env::var_os("TMPDIR")
            && let Ok(root) = physical(Path::new(&root))
        {
            roots.push(root);
        }
        #[cfg(unix)]
        if self.sandbox["excludeSlashTmp"] == false {
            roots.push(physical(Path::new("/tmp"))?);
        }
        if roots.iter().any(|root| path.starts_with(root)) {
            return Ok(());
        }
        Err(format!(
            "caller permissions do not allow worker temporary workspace {}; configure an existing temp directory outside Team records within caller writable scope; no extra write root was granted",
            path.display()
        ))
    }
}

// Resolve existing ancestors, including symlinks, without creating the requested path.
pub(super) fn physical(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("caller/write paths must be absolute and contain no parent traversal".into());
    }
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("cannot verify a dangling symlink write path".into());
            }
            let parent = path.parent().ok_or("cannot resolve write path ancestor")?;
            let name = path.file_name().ok_or("cannot resolve write path name")?;
            Ok(physical(parent)?.join(name))
        }
        Err(error) => Err(format!("resolve caller/write path: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn caller(model: &str, effort: &str) -> Value {
        let mut context = restricted();
        context["model"] = json!(model);
        context["effort"] = json!(effort);
        context
    }

    #[test]
    fn astra_defaults_are_independent_of_caller_effort() {
        for main in ["gpt-6-astra", "gpt-6.2-astra"] {
            for effort in [
                "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
            ] {
                let (model, effort, sources, execution) =
                    resolve(None, None, caller(main, effort)).unwrap();
                assert_eq!((model.as_str(), effort.as_str()), ("gpt-6.1-sol", "high"));
                assert_eq!(sources, json!({"model":"mapping", "effort":"mapping"}));
                assert_eq!(execution.mode, "workspace-write");
            }
        }
    }

    #[test]
    fn overrides_replace_only_the_selected_mapped_field() {
        for (model, effort, expected_model, expected_effort, sources) in [
            (
                Some("custom"),
                None,
                "custom",
                "high",
                json!({"model":"explicit", "effort":"mapping"}),
            ),
            (
                None,
                Some("low"),
                "gpt-6.1-sol",
                "low",
                json!({"model":"mapping", "effort":"explicit"}),
            ),
            (
                Some("custom"),
                Some("low"),
                "custom",
                "low",
                json!({"model":"explicit", "effort":"explicit"}),
            ),
        ] {
            let (model, effort, actual_sources, _) = resolve(
                model.map(str::to_owned),
                effort.map(str::to_owned),
                caller("gpt-6-astra", "ultra"),
            )
            .unwrap();
            assert_eq!(
                (model.as_str(), effort.as_str()),
                (expected_model, expected_effort)
            );
            assert_eq!(actual_sources, sources);
        }
    }

    #[test]
    fn recognized_models_inherit_without_alias_matching() {
        for model in [
            "gpt-6.1-sol",
            "gpt-6-sol",
            "gpt-6-luna",
            "gpt-5.6-sol",
            "claude-haiku-5-5",
            "claude-future-100",
        ] {
            let (actual_model, effort, sources, _) =
                resolve(None, None, caller(model, "medium")).unwrap();
            assert_eq!((actual_model.as_str(), effort.as_str()), (model, "medium"));
            assert_eq!(sources, json!({"model":"parent", "effort":"parent"}));
        }
        let (_, effort, sources, _) =
            resolve(Some("gpt-6-astra".into()), None, caller("gpt-6-sol", "low")).unwrap();
        assert_eq!(effort, "low");
        assert_eq!(sources, json!({"model":"explicit", "effort":"parent"}));
    }

    #[test]
    fn unknown_or_missing_model_identity_uses_fallback_not_caller_effort() {
        for model in [
            json!("other"),
            json!("gpt-6-astra-preview"),
            json!("GPT-6-ASTRA"),
            json!("gpt-6-sol-preview"),
            json!(""),
            Value::Null,
        ] {
            let mut context = caller("unused", "ultra");
            context["model"] = model;
            let (model, effort, sources, _) = resolve(None, None, context).unwrap();
            assert_eq!((model.as_str(), effort.as_str()), ("gpt-6.1-sol", "high"));
            assert_eq!(sources, json!({"model":"fallback", "effort":"fallback"}));
        }
        let (model, effort, sources, _) = resolve(None, None, restricted()).unwrap();
        assert_eq!((model.as_str(), effort.as_str()), ("gpt-6.1-sol", "high"));
        assert_eq!(sources, json!({"model":"fallback", "effort":"fallback"}));
    }

    #[test]
    fn explicit_fields_override_unknown_caller_fallback() {
        for (model, effort, expected_model, expected_effort, sources) in [
            (
                Some("custom"),
                None,
                "custom",
                "high",
                json!({"model":"explicit", "effort":"fallback"}),
            ),
            (
                None,
                Some("low"),
                "gpt-6.1-sol",
                "low",
                json!({"model":"fallback", "effort":"explicit"}),
            ),
            (
                Some("custom"),
                Some("low"),
                "custom",
                "low",
                json!({"model":"explicit", "effort":"explicit"}),
            ),
        ] {
            let (model, effort, actual_sources, _) = resolve(
                model.map(str::to_owned),
                effort.map(str::to_owned),
                caller("claude", "ultra"),
            )
            .unwrap();
            assert_eq!(
                (model.as_str(), effort.as_str()),
                (expected_model, expected_effort)
            );
            assert_eq!(actual_sources, sources);
        }
        assert!(resolve(Some("bad model".into()), None, restricted()).is_err());
        assert!(resolve(None, Some("invalid".into()), restricted()).is_err());
    }

    #[test]
    fn fallback_and_explicit_fields_still_require_verified_permissions() {
        for field in ["approval_policy", "sandbox_policy"] {
            for (model, effort) in [(None, None), (Some("custom"), Some("low"))] {
                let mut context = caller("claude", "medium");
                context[field] = Value::Null;
                assert!(
                    resolve(model.map(str::to_owned), effort.map(str::to_owned), context).is_err(),
                    "{field}"
                );
            }
        }
        assert!(resolve(None, None, Value::Null).is_err());
    }

    #[test]
    fn mapping_and_explicit_overrides_cannot_bypass_permissions_or_validation() {
        for (model, effort) in [(None, None), (Some("custom"), Some("low"))] {
            let mut context = caller("gpt-6-astra", "high");
            context["approval_policy"] = json!("on-request");
            assert!(
                resolve(model.map(str::to_owned), effort.map(str::to_owned), context)
                    .unwrap_err()
                    .contains("approval")
            );
        }
        assert!(
            resolve(
                Some("bad model".into()),
                None,
                caller("gpt-6-astra", "high")
            )
            .is_err()
        );
        assert!(resolve(None, Some("invalid".into()), caller("gpt-6-astra", "high")).is_err());
    }

    fn restricted() -> Value {
        json!({"approval_policy":"never", "cwd":std::env::current_dir().unwrap(),
            "sandbox_policy":{"type":"workspace-write","writable_roots":[],
                "network_access":false,"exclude_tmpdir_env_var":true,"exclude_slash_tmp":true}})
    }

    #[test]
    fn full_access_is_not_restricted_by_worker_defaults() {
        let context = json!({"approval_policy":"never", "sandbox_policy":{"type":"danger-full-access"},
            "permission_profile":{"type":"managed","file_system":{"type":"unrestricted"},"network":"enabled"},
            "file_system_sandbox_policy":{"type":"unrestricted"}});
        let full = Execution::from_context(&context).unwrap();
        assert_eq!(full.mode, "danger-full-access");
        assert_eq!(full.sandbox, json!({"type":"dangerFullAccess"}));
        assert_eq!(crate::harness::codex::execution_config(&full), json!({}));
        full.require_writable(Path::new("/any/task-authorized/location"))
            .unwrap();
    }

    #[test]
    fn authentic_disabled_profile_requires_consistent_full_access() {
        // Authentic latest caller metadata supplied by main; no filesystem policy field.
        let context = json!({"approval_policy":"never", "sandbox_policy":{"type":"danger-full-access"},
            "permission_profile":{"type":"disabled"}});
        let execution = Execution::from_context(&context).unwrap();
        assert_eq!(execution.sandbox, json!({"type":"dangerFullAccess"}));
        assert_eq!(
            crate::harness::codex::execution_config(&execution),
            json!({})
        );
        let mut restricted = restricted();
        restricted["permission_profile"] = context["permission_profile"].clone();
        assert!(
            Execution::from_context(&restricted)
                .unwrap_err()
                .contains("permission_profile")
        );
        for profile in [
            json!({"type":"unknown"}),
            json!("disabled"),
            json!({"type":"disabled", "network":"restricted"}),
            json!({"type":"disabled", "file_system":{"type":"restricted"}}),
            json!({"type":"disabled", "unexpected":true}),
        ] {
            let mut conflicting = context.clone();
            conflicting["permission_profile"] = profile;
            assert!(
                Execution::from_context(&conflicting)
                    .unwrap_err()
                    .contains("permission_profile")
            );
        }
        let mut conflicting = context.clone();
        conflicting["file_system_sandbox_policy"] = json!({"type":"restricted"});
        assert!(
            Execution::from_context(&conflicting)
                .unwrap_err()
                .contains("file_system_sandbox_policy")
        );
        conflicting = context.clone();
        conflicting["sandbox_policy"]["network_access"] = json!(false);
        assert!(Execution::from_context(&conflicting).is_err());
    }

    #[test]
    fn caller_roots_network_and_temp_flags_are_preserved() {
        let mut context = restricted();
        let cwd = fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        for network in [false, true] {
            context["sandbox_policy"]["network_access"] = json!(network);
            context["sandbox_policy"]["exclude_slash_tmp"] = json!(!network);
            let execution = Execution::from_context(&context).unwrap();
            assert_eq!(
                execution.sandbox,
                json!({"type":"workspaceWrite","writableRoots":[cwd],
                "networkAccess":network,"excludeTmpdirEnvVar":true,"excludeSlashTmp":!network})
            );
            assert_eq!(
                crate::harness::codex::execution_config(&execution)["sandbox_workspace_write"]["network_access"],
                network
            );
            execution
                .require_writable(&cwd.join("task-local-future-path"))
                .unwrap();
            assert!(
                execution
                    .require_writable(&cwd.join(".git/config"))
                    .is_err()
            );
            assert!(
                execution
                    .require_writable(&cwd.parent().unwrap().join("outside-caller-scope"))
                    .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn write_aliases_cannot_bypass_caller_roots() {
        let root = std::env::temp_dir().join(format!(
            "team-write-alias-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let allowed = root.join("allowed");
        let outside = root.join("outside");
        fs::create_dir_all(&allowed).unwrap();
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, allowed.join("alias")).unwrap();
        std::os::unix::fs::symlink(outside.join("missing"), allowed.join("dangling")).unwrap();
        let mut context = restricted();
        context["cwd"] = json!(allowed);
        let execution = Execution::from_context(&context).unwrap();
        assert!(
            execution
                .require_writable(&allowed.join("alias/future"))
                .is_err()
        );
        assert!(
            execution
                .require_writable(&allowed.join("dangling/future"))
                .is_err()
        );
        execution.require_writable(&allowed.join("future")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn no_approval_or_unknown_policy_can_be_inferred_from_environment_or_old_permissions() {
        for policy in [
            Value::Null,
            json!({"type":"unknown"}),
            json!({"type":"external-sandbox"}),
            json!({"type":"read-only"}),
            json!({"type":"danger-full-access","network_access":false}),
        ] {
            let mut context = restricted();
            context["sandbox_policy"] = policy;
            assert!(Execution::from_context(&context).is_err());
        }
        for approval in [
            Value::Null,
            json!("on-request"),
            json!("untrusted"),
            json!({"granular":{}}),
        ] {
            let mut context = restricted();
            context["approval_policy"] = approval;
            assert!(Execution::from_context(&context).is_err());
        }
        for field in [
            "network_access",
            "writable_roots",
            "exclude_tmpdir_env_var",
            "exclude_slash_tmp",
        ] {
            let mut context = restricted();
            context["sandbox_policy"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(Execution::from_context(&context).is_err(), "{field}");
        }
        for field in ["permission_profile", "file_system_sandbox_policy"] {
            let mut context = restricted();
            context[field] = json!({"type":"restricted"});
            assert!(Execution::from_context(&context).is_err(), "{field}");
        }
    }
}
