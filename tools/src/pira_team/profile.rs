// Adapted from the archived Team profile reader: inherit settings, never parent messages.
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SESSION_BYTES: u64 = 256 * 1024 * 1024;
const WORKER_PROFILES: &str = include_str!("worker_profiles.json");

#[path = "defaults.rs"]
pub mod defaults;

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
    let overrides = defaults::load(store)?;
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
            if caller_model
                .is_some_and(|model| inherit.iter().any(|entry| entry.as_str() == Some(model))) =>
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

pub fn load() -> Result<Value, String> {
    let thread = std::env::var("CODEX_THREAD_ID").map_err(|_| "CODEX_THREAD_ID is unavailable")?;
    if thread.is_empty()
        || thread.len() > 200
        || !thread
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("invalid CODEX_THREAD_ID".into());
    }
    let home = std::env::var_os("CODEX_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            ["HOME", "USERPROFILE"]
                .iter()
                .find_map(|key| std::env::var_os(key).filter(|value| !value.is_empty()))
                .map(|p| PathBuf::from(p).join(".codex"))
        })
        .ok_or("cannot locate Codex home")?;
    let sessions = home.join("sessions");
    let mut matches = Vec::new();
    collect(
        &sessions,
        &format!("-{thread}.jsonl"),
        0,
        &mut 0,
        &mut matches,
    )?;
    if matches.len() != 1 {
        return Err("expected one matching parent session log".into());
    }
    read(&matches[0], &thread)
}

fn collect(
    dir: &Path,
    suffix: &str,
    depth: usize,
    visited: &mut usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if depth > 8 {
        return Err("session tree exceeds depth limit".into());
    }
    let metadata = fs::symlink_metadata(dir).map_err(|_| "cannot inspect session directory")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("session directory must not be a symlink".into());
    }
    for entry in fs::read_dir(dir).map_err(|_| "cannot read session directory")? {
        let entry = entry.map_err(|_| "cannot read session entry")?;
        *visited += 1;
        if *visited > 200_000 {
            return Err("session tree exceeds entry limit".into());
        }
        let kind = entry
            .file_type()
            .map_err(|_| "cannot inspect session entry")?;
        if kind.is_dir() {
            collect(&entry.path(), suffix, depth + 1, visited, matches)?;
        } else if kind.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(suffix))
        {
            matches.push(entry.path());
            if matches.len() > 1 {
                return Err("ambiguous parent session logs".into());
            }
        }
        // Do not follow symlinks or read any nonmatching conversation file.
    }
    Ok(())
}

fn read(path: &Path, thread: &str) -> Result<Value, String> {
    let file = File::open(path).map_err(|_| "cannot open parent session log")?;
    let len = file
        .metadata()
        .map_err(|_| "cannot inspect parent session log")?
        .len();
    if len > MAX_SESSION_BYTES {
        return Err("parent session log exceeds 256 MiB".into());
    }
    // A finite snapshot prevents an actively growing session from extending this read forever.
    let mut reader = BufReader::new(file.take(len));
    let mut latest = None;
    let mut matched = false;
    loop {
        let mut line = Vec::new();
        let size = reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| "cannot read parent session log")?;
        if size == 0 {
            break;
        }
        if size as u64 > MAX_RECORD_BYTES {
            return Err("parent record exceeds 16 MiB".into());
        }
        if line.last() != Some(&b'\n') {
            return Err("parent log has an incomplete record; retry after it is flushed".into());
        }
        let record: Value =
            serde_json::from_slice(&line).map_err(|_| "malformed parent session record")?;
        match record["type"].as_str() {
            Some("session_meta") => {
                if record["payload"]["id"].as_str() != Some(thread) {
                    return Err("parent session identity mismatch".into());
                }
                matched = true;
                latest = None;
            }
            Some("turn_context") if matched => {
                // Replace the entire context atomically: never backfill fields from an older turn.
                latest = Some(
                    json!({"model": record["payload"]["model"], "effort": record["payload"]["effort"],
                        "cwd":record["payload"]["cwd"], "sandbox_policy":record["payload"]["sandbox_policy"],
                        "approval_policy":record["payload"]["approval_policy"],
                        "permission_profile":record["payload"]["permission_profile"],
                        "file_system_sandbox_policy":record["payload"]["file_system_sandbox_policy"],
                        "caller_thread_id":thread}),
                );
            }
            _ => {}
        }
    }
    if !matched {
        return Err("parent session identity was not verified".into());
    }
    latest.ok_or_else(|| "parent session has no recorded turn context".into())
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

    pub fn config(&self) -> Value {
        if self.mode == "workspace-write" {
            json!({"sandbox_workspace_write":{
                "writable_roots":self.sandbox["writableRoots"],
                "network_access":self.sandbox["networkAccess"],
                "exclude_tmpdir_env_var":self.sandbox["excludeTmpdirEnvVar"],
                "exclude_slash_tmp":self.sandbox["excludeSlashTmp"]}})
        } else {
            json!({})
        }
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
}

// Resolve existing ancestors, including symlinks, without creating the requested path.
fn physical(path: &Path) -> Result<PathBuf, String> {
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
        for effort in [
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ] {
            let (model, effort, sources, execution) =
                resolve(None, None, caller("gpt-6-astra", effort)).unwrap();
            assert_eq!((model.as_str(), effort.as_str()), ("gpt-6.1-sol", "high"));
            assert_eq!(sources, json!({"model":"mapping", "effort":"mapping"}));
            assert_eq!(execution.mode, "workspace-write");
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
        for model in ["gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna", "gpt-5.6-sol"] {
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
            json!("claude-sonnet"),
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
        assert_eq!(full.config(), json!({}));
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
        assert_eq!(execution.config(), json!({}));
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
                execution.config()["sandbox_workspace_write"]["network_access"],
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
    fn latest_verified_context_replaces_full_access_without_backfilling_resume() {
        let path = std::env::temp_dir().join(format!(
            "team-profile-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let records = |latest: Value| {
            format!(
                "{}\n{}\n{}\n{}\n",
                json!({"type":"session_meta","payload":{"id":"caller"}}),
                json!({"type":"turn_context","payload":{"approval_policy":"never","sandbox_policy":{"type":"danger-full-access"}}}),
                json!({"type":"response_item","payload":{"sandbox_policy":{"type":"danger-full-access"},"text":"not settings"}}),
                json!({"type":"turn_context","payload":latest})
            )
        };
        fs::write(&path, records(restricted())).unwrap();
        let context = read(&path, "caller").unwrap();
        assert!(read(&path, "unrelated").is_err());
        let (model, effort, sources, execution) =
            resolve(Some("retained-model".into()), Some("high".into()), context).unwrap();
        assert_eq!(
            (model.as_str(), effort.as_str()),
            ("retained-model", "high")
        );
        assert_eq!(sources, json!({"model":"explicit", "effort":"explicit"}));
        assert_eq!(execution.mode, "workspace-write");
        assert_eq!(execution.caller_thread_id, "caller");
        fs::write(&path, records(json!({"model":"current","effort":"high"}))).unwrap();
        assert!(Execution::from_context(&read(&path, "caller").unwrap()).is_err());
        fs::remove_file(path).unwrap();
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
