//! Native Codex launch, isolated home, schema preflight and caller adaptation.
mod caller;
mod caller_log;
mod schema;

pub(super) use caller::{caller_context, caller_identity};
pub(crate) use schema::resume_params;
#[cfg(test)]
pub(crate) use schema::tests::installed_native_schema_supports_team_requests;
pub(super) use schema::{preflight, validate_request};

use super::worker_path;
use crate::authentication::selection;
use crate::{Options, authentication, lifecycle, notice, private_dir};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) struct IsolatedHome(PathBuf);

impl IsolatedHome {
    pub(super) fn new(
        run: &Path,
        authentication: &authentication::Authentication,
    ) -> Result<Self, String> {
        let path = run.join("codex-home");
        match private_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                lifecycle::private_path(&path, true)?
            }
            Err(e) => return Err(format!("create isolated Codex home: {e}")),
        }
        // Remove only a stale launcher-owned link, never its credential target.
        let auth = path.join("auth.json");
        if fs::symlink_metadata(&auth).is_ok() {
            fs::remove_file(auth).map_err(|e| format!("remove stale auth link: {e}"))?;
        }
        let config = path.join("config.toml");
        if fs::symlink_metadata(&config).is_ok() {
            fs::remove_file(&config).map_err(|_| "remove stale selected provider config")?;
        }
        let home = Self(path);
        if !authentication.provider_config().is_empty() {
            use std::io::Write;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options
                .open(config)
                .and_then(|mut file| file.write_all(authentication.provider_config().as_bytes()))
                .map_err(|_| "write isolated selected provider configuration (values withheld)")?;
        }
        if authentication.uses_api_key() {
            return Ok(home);
        }
        let source = match authentication.file_source() {
            Some(source) => Some(source.to_owned()),
            None if matches!(authentication, authentication::Authentication::Caller) => {
                authentication::caller_file()?
            }
            None => None,
        };
        if let Some(source) = source {
            // Link authentication only; provider config above is an allowlisted projection.
            selection::link(&source, &home.0.join("auth.json"))?;
        }
        Ok(home)
    }
    fn detach(&self, warn: impl FnOnce(std::fmt::Arguments<'_>)) {
        // Retain only the worker's session; detach authentication after every revision.
        let mut failure = None;
        for name in ["auth.json", "config.toml"] {
            if let Err(error) = fs::remove_file(self.0.join(name))
                && error.kind() != std::io::ErrorKind::NotFound
            {
                failure = Some(error);
            }
        }
        if let Some(error) = failure {
            warn(format_args!(
                "pira_team: detach worker authentication/config: {error}"
            ));
        }
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        self.detach(notice);
    }
}

// Codex CLI/app-server keys encode an already verified shared permission policy.
pub(crate) fn execution_config(execution: &crate::profile::Execution) -> Value {
    if execution.mode == "workspace-write" {
        json!({"sandbox_workspace_write":{
                "writable_roots":execution.sandbox["writableRoots"],
                "network_access":execution.sandbox["networkAccess"],
                "exclude_tmpdir_env_var":execution.sandbox["excludeTmpdirEnvVar"],
                "exclude_slash_tmp":execution.sandbox["excludeSlashTmp"]}})
    } else {
        json!({})
    }
}

pub(crate) fn command(options: &Options, run: &Path, dir: &Path, handoff: &Path) -> Command {
    let mut cmd = Command::new(options.authentication.program());
    options.authentication.apply(&mut cmd);
    cmd.args(["app-server", "--strict-config", "--stdio"]);
    for setting in schema::contract()["config"].as_array().unwrap() {
        cmd.arg("-c").arg(setting.as_str().unwrap());
    }
    for (key, value) in execution_config(&options.execution).as_object().unwrap() {
        for (name, value) in value.as_object().unwrap() {
            cmd.arg("-c").arg(format!("{key}.{name}={value}"));
        }
    }
    for setting in [
        format!("sandbox_mode={}", json!(options.execution.mode)),
        "approval_policy=\"never\"".to_owned(),
        "features.fast_mode=true".to_owned(),
        format!("model={}", json!(options.model)),
        format!("model_reasoning_effort={}", json!(options.effort)),
        format!("model_instructions_file={}", json!(dir.join("policy.md"))),
        format!(
            "projects={{{}={{trust_level=\"untrusted\"}}}}",
            // CLI keys split on every dot; only the value supports TOML quoted keys.
            // JSON strings are TOML basic strings except for literal DEL.
            json!(options.cwd.to_string_lossy())
                .to_string()
                .replace('\u{7f}', "\\u007f")
        ),
    ] {
        cmd.arg("-c").arg(setting);
    }
    for (key, value) in crate::worker_context_environment(options, run, dir, handoff) {
        cmd.arg("-c").arg(format!(
            "shell_environment_policy.set.{key}={}",
            json!(value)
        ));
    }
    // Keep unknown task configuration without serializing its values into argv/logs.
    // Backend authentication remains available to Codex, not to worker shells.
    cmd.arg("-c")
        .arg("shell_environment_policy.ignore_default_excludes=true");
    let mut exclusions = vec![
        "CODEX_HOME",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "CLAUDE_CODE_SESSION_ID",
        "PIRA_CTX_THREAD_ID",
        "PIRA_TEAM_DIR",
        "PIRA_TEAM_HANDOFF",
        "PIRA_TEAM_CHILD",
        "PIRA_TEAM_CONTEXT",
        "PIRA_TEAM_ATTEMPT",
        "PIRA_CTX_STORE_DIR",
        "PIRA_DEC_STORE_DIR",
        "PIRA_DEC_WORKSPACE_DIR",
        "CODEX_API_KEY",
        "OPENAI_API_KEY",
        "OPENAI_ADMIN_KEY",
        "ANTHROPIC_*",
        "CLAUDE_*",
        "*_API_KEY",
        "*_TOKEN",
        "*_SECRET",
        "*_SECRET_KEY",
        "*_PASSWORD",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "SSH_AUTH_SOCK",
        "SSH_AGENT_PID",
    ];
    exclusions.extend(
        options
            .authentication
            .provider_environment()
            .iter()
            .map(String::as_str),
    );
    cmd.arg("-c").arg(format!(
        "shell_environment_policy.exclude={}",
        json!(exclusions)
    ));
    for key in [
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "PIRA_CTX_THREAD_ID",
        "CLAUDE_CODE_SESSION_ID",
    ] {
        cmd.env_remove(key);
    }
    for (key, path) in crate::worker_temp_environment(options) {
        cmd.env(key, &path);
        cmd.arg("-c").arg(format!(
            "shell_environment_policy.set.{key}={}",
            json!(path)
        ));
    }
    cmd.env("PIRA_TEAM_CHILD", "1");
    cmd.env("CODEX_HOME", run.join("codex-home"));
    cmd.current_dir(dir).stdin(Stdio::piped());
    cmd
}

pub(super) fn set_worker_launcher(command: &mut Command) -> Result<(), String> {
    let launcher = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|_| "cannot resolve current Team launcher for worker shells")?;
    let path = worker_path(&launcher, std::env::var_os("PATH").as_deref())?;
    // Match executable-bound child authentication without changing the backend
    // process PATH or its independently retained executable/provider selection.
    command.arg("-c").arg(format!(
        "shell_environment_policy.set.PATH={}",
        json!(path).to_string().replace('\u{7f}', "\\u007f")
    ));
    Ok(())
}

pub(crate) fn relevant_event(value: &Value) -> bool {
    value.get("id").is_some()
        || match value["method"].as_str() {
            Some("turn/completed" | "thread/tokenUsage/updated") => true,
            Some("item/completed") => {
                let item = &value["params"]["item"];
                item["type"] == "agentMessage"
                    && (item["phase"].is_null() || item["phase"] == "final_answer")
            }
            _ => false,
        }
}

#[cfg(test)]
#[path = "tests/launch.rs"]
mod launch_tests;
