//! Native Claude Code support. No SDK or auxiliary runtime is required.
use crate::{Options, lifecycle};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(crate) fn relevant_event(value: &Value) -> bool {
    matches!(
        value["type"].as_str(),
        Some("system" | "result" | "control_response" | "control_request" | "user")
    )
}

pub(crate) fn uuid() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn excluded_shell_variable(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    ["_API_KEY", "_TOKEN", "_SECRET", "_SECRET_KEY", "_PASSWORD"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
        || (name.starts_with("CLAUDE_") && name != "CLAUDE_CODE_SESSION_ID")
        || name.starts_with("ANTHROPIC_")
        || name.starts_with("OPENAI_")
        || name.starts_with("AZURE_OPENAI_")
        || [
            "CODEX_HOME",
            "CODEX_THREAD_ID",
            "CODEX_SESSION_ID",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_SHELL_PREFIX",
            "PIRA_CTX_THREAD_ID",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "SSH_AUTH_SOCK",
            "SSH_AGENT_PID",
        ]
        .contains(&name.as_str())
}

// Claude's documented shell prefix receives the complete shell invocation as one
// argument. It does not grant authority; it only removes inherited credentials.
pub(crate) fn shell_entry() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    let name = Path::new(args.first()?).file_stem()?.to_str()?;
    if name != "pira-team-claude-shell" {
        return None;
    }
    if args.len() != 2 {
        return Some(2);
    }
    let mut command = Command::new("bash");
    command.args([std::ffi::OsStr::new("-c"), &args[1]]);
    for (key, _) in std::env::vars_os() {
        if excluded_shell_variable(&key.to_string_lossy()) {
            command.env_remove(key);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        crate::notice(format_args!(
            "pira_team: Claude shell launch failed: {error}"
        ));
        Some(1)
    }
    #[cfg(not(unix))]
    {
        Some(command.status().ok().and_then(|s| s.code()).unwrap_or(1))
    }
}

#[cfg(any(windows, test))]
fn link_shell_filter(
    source: &Path,
    destination: &Path,
    link: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match link(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            let mut input = fs::File::open(source)?;
            // Unlike fs::copy, never overwrite a preexisting file or follow a
            // destination symlink. Failed copies stay in the failed attempt.
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)?;
            std::io::copy(&mut input, &mut output)?;
            output.sync_all()
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn command(
    options: &Options,
    run: &Path,
    dir: &Path,
    handoff: &Path,
) -> Result<Command, String> {
    // Never translate a narrower OS policy into Claude's permission bypass.
    if options.execution.mode != "danger-full-access" {
        return Err("Claude workers require verified full-access permission; workspace-write cannot be represented by Claude's tool permissions".into());
    }
    if options.service_tier != crate::service_tier::Tier::Standard {
        return Err(
            "Claude service-tier selection is not supported; use --service-tier standard".into(),
        );
    }
    let manifest = lifecycle::read_json(&run.join("manifest.json"))?;
    let session = manifest["thread_id"]
        .as_str()
        .ok_or("missing Claude session ID")?;
    let launcher = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| e.to_string())?;
    let wrapper = dir.join(if cfg!(windows) {
        "pira-team-claude-shell.exe"
    } else {
        "pira-team-claude-shell"
    });
    #[cfg(unix)]
    std::os::unix::fs::symlink(&launcher, &wrapper)
        .map_err(|e| format!("create Claude shell filter: {e}"))?;
    #[cfg(windows)]
    link_shell_filter(&launcher, &wrapper, |source, destination| {
        fs::hard_link(source, destination)
    })
    .map_err(|e| format!("create Claude shell filter: {e}"))?;
    let path = crate::worker_path(&launcher, std::env::var_os("PATH").as_deref())?;
    let mut environment = json!({
        "PIRA_TEAM_CONTEXT":run, "PIRA_TEAM_CHILD":"1", "PIRA_TEAM_HANDOFF":handoff,
        "PIRA_CTX_STORE_DIR":options.ctx_store,"PIRA_DEC_STORE_DIR":options.dec_store,
        "PATH":path,"CLAUDE_CODE_SHELL_PREFIX":wrapper,
        "CLAUDE_CODE_EFFORT_LEVEL":options.effort,
        "CLAUDE_CODE_SKIP_PROMPT_HISTORY":"0",
        "CLAUDE_CODE_DISABLE_AUTO_MEMORY":"1",
        "CLAUDE_CODE_SDK_READS_SESSION_STATE":"1",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":"1"
    });
    if let Some(workspace) = &options.dec_workspace {
        environment["PIRA_DEC_WORKSPACE_DIR"] = json!(workspace);
    }
    let scratch = run.join("scratch");
    for key in ["TMPDIR", "TEMP", "TMP"] {
        environment[key] = json!(scratch);
    }
    environment["TMPPREFIX"] = json!(scratch.join("zsh"));
    let settings = json!({"env":environment,"sandbox":{"enabled":false},"fastMode":false});
    let mut command = Command::new(options.authentication.program());
    options.authentication.apply(&mut command);
    command.args([
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--replay-user-messages",
        "--safe-mode",
        "--setting-sources",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--no-chrome",
        "--permission-mode",
        "bypassPermissions",
        "--tools",
        "Bash,Read,Write,Edit,Glob,Grep",
        "--system-prompt-snapshot",
        "off",
        "--model",
        &options.model,
        "--effort",
        &options.effort,
    ]);
    command.arg("--settings").arg(settings.to_string());
    command.arg("--system-prompt").arg(format!(
        "{}\n\n{}",
        fs::read_to_string(dir.join("policy.md")).map_err(|e| e.to_string())?,
        fs::read_to_string(dir.join("phase.md")).map_err(|e| e.to_string())?
    ));
    command
        .arg(if manifest["claude_session_started"] == true {
            "--resume"
        } else {
            "--session-id"
        })
        .arg(session);
    for key in [
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDECODE",
        "PIRA_CTX_THREAD_ID",
        "PIRA_TEAM_CONTEXT",
        "PIRA_TEAM_CHILD",
        "PIRA_DEC_WORKSPACE_DIR",
    ] {
        command.env_remove(key);
    }
    // Explicitly neutralize inherited feature switches that can re-enable ambient
    // prompts, suppress native session persistence, or override the selected effort.
    for (key, value) in environment.as_object().unwrap() {
        command.env(
            key,
            value
                .as_str()
                .ok_or("non-string Claude environment value")?,
        );
    }
    command
        .env_remove("CLAUDE_CODE_SIMPLE")
        .env_remove("CLAUDE_CODE_SUBAGENT_MODEL");
    command.current_dir(&options.cwd).stdin(Stdio::piped());
    Ok(command)
}

pub(crate) fn request(id: &str, subtype: &str) -> Value {
    json!({"type":"control_request", "request_id":id,"request":{"subtype":subtype}})
}

pub(crate) fn user_message(id: &str, session: &str, text: &str) -> Value {
    json!({"type":"user","uuid":id,"session_id":session,"parent_tool_use_id":null,
        "client_composed":true,"message":{"role":"user","content":text}})
}

pub(crate) fn usage(value: &Value, baseline: &Value) -> Option<Value> {
    let read = value["cache_read_input_tokens"].as_u64()?;
    let write = value["cache_creation_input_tokens"].as_u64()?;
    let input = value["input_tokens"]
        .as_u64()?
        .checked_add(read)?
        .checked_add(write)?;
    let output = value["output_tokens"].as_u64()?;
    let mut total = json!({});
    for (key, amount) in [
        ("input_tokens", input),
        ("cached_input_tokens", read),
        ("output_tokens", output),
    ] {
        total[key] = json!(baseline[key].as_u64().unwrap_or(0).checked_add(amount)?);
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_secrets_but_keeps_native_identity() {
        for name in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_CUSTOM_KEY",
            "SSH_AUTH_SOCK",
        ] {
            assert!(excluded_shell_variable(name));
        }
        for name in ["PATH", "CLAUDE_CODE_SESSION_ID", "PIRA_TEAM_CONTEXT"] {
            assert!(!excluded_shell_variable(name));
        }
    }
    #[test]
    fn usage_counts_cache_creation_and_resumed_attempts() {
        let usage = json!({"input_tokens":2,"cache_read_input_tokens":10,"cache_creation_input_tokens":5,"output_tokens":3});
        assert_eq!(
            super::usage(&usage, &json!({"input_tokens":100,"output_tokens":7})),
            Some(json!({"input_tokens":117,"cached_input_tokens":10,"output_tokens":10}))
        );
        assert_eq!(super::usage(&json!({}), &json!({})), None);
    }
}

pub(crate) fn is_executable(path: &Path) -> bool {
    crate::authentication::resolve_program_for(crate::harness::Harness::Claude)
        .is_ok_and(|selected| selected_image_matches(&selected, path))
}

fn selected_image_matches(selected: &Path, image: &Path) -> bool {
    // Windows image queries use Win32 paths; selected executables are canonical
    // verbatim paths. Compare the same representation without basename matching.
    image.canonicalize().is_ok_and(|image| selected == image)
}

pub(crate) fn caller_context(session: &str) -> Result<Value, String> {
    crate::profile::validate_session_id(session)?;
    let worker = crate::subteam::caller_run()?;
    let mut pid = std::process::id();
    let mut birth = u64::MAX;
    let mut verified = false;
    for _ in 0..64 {
        let identity =
            crate::native::identity(pid).map_err(|_| "cannot verify Claude caller ancestry")?;
        if !identity.alive || identity.birth > birth {
            break;
        }
        if is_executable(&identity.executable) {
            verified = true;
            if !crate::native::claude_shell_unsandboxed(pid)
                .map_err(|e| format!("cannot verify current Claude shell sandbox: {e}"))?
            {
                return Err("Claude Team requires an unsandboxed full-access shell; current command is sandboxed".into());
            }
            if crate::native::identity(pid)
                .map_err(|_| "Claude caller exited during verification")?
                != identity
            {
                return Err("Claude caller identity changed during verification".into());
            }
            break;
        }
        birth = identity.birth;
        pid = identity.parent;
        if pid <= if cfg!(windows) { 4 } else { 1 } {
            break;
        }
    }
    if !verified {
        return Err("CLAUDE_CODE_SESSION_ID requires a native Claude ancestor, not an environment assertion".into());
    }
    if let Some(run) = worker {
        let manifest = lifecycle::read_json(&run.join("manifest.json"))?;
        if manifest["transport"] != "claude-stream-json" || manifest["thread_id"] != session {
            return Err("Claude caller differs from retained native worker identity".into());
        }
        if manifest["sandbox"] != "danger-full-access" {
            return Err("unsupported retained Claude permissions".into());
        }
        return Ok(
            json!({"caller_thread_id":format!("claude-{session}"),"model":manifest["model"],"effort":manifest["effort"],
            "cwd":manifest["cwd"],"approval_policy":"never","sandbox_policy":{"type":"danger-full-access"}}),
        );
    }
    let home = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|p| PathBuf::from(p).join(".claude"))
        })
        .ok_or("cannot locate native Claude session directory")?;
    let projects = home.join("projects");
    let mut found = None;
    for (index, entry) in fs::read_dir(&projects)
        .map_err(|_| "cannot inspect Claude projects")?
        .enumerate()
    {
        if index >= 200_000 {
            return Err("Claude project scan exceeds entry limit".into());
        }
        let entry = entry.map_err(|_| "cannot inspect Claude project")?;
        if !entry
            .file_type()
            .map_err(|_| "cannot inspect Claude project type")?
            .is_dir()
        {
            continue;
        }
        let candidate = entry.path().join(format!("{session}.jsonl"));
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                if found.replace(candidate).is_some() {
                    return Err("ambiguous Claude session transcripts".into());
                }
            }
            Ok(_) => return Err("Claude transcript is not a regular file".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("cannot inspect Claude transcript".into()),
        }
    }
    let path = found.ok_or("native Claude session transcript unavailable")?;
    read_context(&path, session)
}

fn read_context(path: &Path, session: &str) -> Result<Value, String> {
    use std::io::{BufRead, BufReader, Read};
    let file = fs::File::open(path).map_err(|_| "cannot read matching Claude transcript")?;
    let length = file
        .metadata()
        .map_err(|_| "cannot inspect Claude transcript length")?
        .len();
    let mut reader = BufReader::new(file.take(length));
    let mut context = json!({"caller_thread_id":format!("claude-{session}")});
    let mut matched = false;
    loop {
        let mut line = Vec::new();
        let n = reader
            .by_ref()
            .take(16 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| "cannot read Claude transcript record")?;
        if n == 0 {
            break;
        }
        if n > 16 * 1024 * 1024 {
            return Err("Claude transcript record exceeds limit".into());
        }
        if line.last() != Some(&b'\n') {
            return Err(
                "incomplete Claude caller transcript; cannot verify current permissions".into(),
            );
        }
        let value: Value =
            serde_json::from_slice(&line).map_err(|_| "malformed Claude transcript record")?;
        if value.get("sessionId").is_some() && value["sessionId"] != session {
            return Err("Claude transcript session mismatch".into());
        }
        if value["sessionId"] != session {
            continue;
        }
        matched = true;
        let content = &value["message"]["content"];
        let tool_result = content
            .as_array()
            .is_some_and(|a| a.iter().any(|part| part["type"] == "tool_result"));
        if value["type"] == "user" && !tool_result {
            context = json!({"caller_thread_id":format!("claude-{session}"),"cwd":value["cwd"],
                "permission_mode":value["permissionMode"]});
        } else if value["type"] == "assistant" {
            context["model"] = value["requestedModel"]
                .as_str()
                .map(|s| json!(s))
                .unwrap_or_else(|| value["message"]["model"].clone());
            context["effort"] = value["effort"].clone();
        }
    }
    if !matched || context["permission_mode"] != "bypassPermissions" {
        return Err("Claude caller requires a current verified bypassPermissions turn; interactive and unknown approvals are not inherited".into());
    }
    context["approval_policy"] = json!("never");
    context["sandbox_policy"] = json!({"type":"danger-full-access"});
    Ok(context)
}

#[cfg(test)]
mod caller_tests {
    use super::*;
    use std::io::Write;
    fn read(records: &[Value]) -> Result<Value, String> {
        let path =
            std::env::temp_dir().join(format!("pira-claude-context-{}.jsonl", uuid().unwrap()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        for record in records {
            writeln!(file, "{record}").unwrap();
        }
        drop(file);
        let result = read_context(&path, "native-session");
        fs::remove_file(path).unwrap();
        result
    }
    fn user(mode: Value) -> Value {
        json!({"type":"user","sessionId":"native-session","cwd":"/work","permissionMode":mode,"message":{"content":"task"}})
    }
    fn assistant() -> Value {
        json!({"type":"assistant","sessionId":"native-session","requestedModel":"claude-future-100","effort":"low"})
    }
    fn tool(sandboxed: bool) -> Value {
        json!({"type":"user","sessionId":"native-session","sandboxed":sandboxed,"message":{"content":[{"type":"tool_result"}]}})
    }
    #[test]
    fn verifies_current_native_context_without_private_account_fields() {
        let context = read(&[user(json!("bypassPermissions")), assistant(), tool(false)]).unwrap();
        assert_eq!(context["caller_thread_id"], "claude-native-session");
        assert_eq!(context["model"], "claude-future-100");
        assert_eq!(context["permission_mode"], "bypassPermissions");
    }
    #[test]
    fn new_turn_does_not_inherit_stale_permissions_or_model() {
        assert!(
            read(&[
                user(json!("bypassPermissions")),
                assistant(),
                tool(false),
                user(Value::Null)
            ])
            .is_err()
        );
        let context = read(&[
            user(json!("bypassPermissions")),
            assistant(),
            tool(false),
            user(json!("bypassPermissions")),
        ])
        .unwrap();
        assert_eq!(context["permission_mode"], "bypassPermissions");
        assert_eq!(context["model"], Value::Null);
    }
    #[test]
    fn rejects_other_sessions_and_restricted_approval() {
        assert!(read(&[user(json!("plan"))]).is_err());
        assert!(
            read(&[
                user(json!("bypassPermissions")),
                json!({"sessionId":"other"})
            ])
            .is_err()
        );
        let context = read(&[user(json!("bypassPermissions")), assistant(), tool(true)]).unwrap();
        assert_eq!(context["permission_mode"], "bypassPermissions");
    }
    #[test]
    fn protocol_envelopes_do_not_expand_user_text_as_commands() {
        let message = user_message("message", "session", "/compact @file");
        assert_eq!(message["client_composed"], true);
        assert_eq!(message["message"]["content"], "/compact @file");
        assert_eq!(request("a", "interrupt")["request"]["subtype"], "interrupt");
    }
}

pub(crate) fn result_error(value: &Value) -> String {
    if let Some(errors) = value["errors"].as_array() {
        let messages: Vec<_> = errors.iter().filter_map(Value::as_str).collect();
        if !messages.is_empty() {
            return messages.join("\n");
        }
    }
    if let Some(text) = value["result"].as_str().filter(|s| !s.trim().is_empty()) {
        return text.to_owned();
    }
    value["subtype"]
        .as_str()
        .unwrap_or("Claude CLI returned an error")
        .to_owned()
}

pub(crate) fn native_stderr(dir: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(dir.join("stderr.log")).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(64 * 1024);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(64 * 1024).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod launch_evidence_tests {
    use super::*;
    #[test]
    fn native_errors_are_preserved_without_reinterpretation() {
        assert_eq!(
            result_error(
                &json!({"subtype":"success","is_error":true,"result":"original quota error"})
            ),
            "original quota error"
        );
        assert_eq!(
            result_error(&json!({"errors":["original auth error","detail"]})),
            "original auth error\ndetail"
        );
    }
}

#[cfg(test)]
mod filesystem_boundary_tests {
    use super::*;
    use std::io;

    fn fixture(test: impl FnOnce(&Path, &Path)) {
        let root = std::env::temp_dir().join(format!("team-claude-files-{}", uuid().unwrap()));
        crate::private_dir(&root).unwrap();
        let source = root.join("launcher");
        fs::write(&source, b"synthetic executable image").unwrap();
        test(&source, &root.join("filter"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filter_keeps_same_volume_hard_link_path() {
        fixture(|source, destination| {
            link_shell_filter(source, destination, |source, destination| {
                fs::hard_link(source, destination)
            })
            .unwrap();
            assert_eq!(fs::read(destination).unwrap(), fs::read(source).unwrap());
        });
    }

    #[test]
    fn filter_copies_exact_image_when_link_crosses_volumes() {
        fixture(|source, destination| {
            link_shell_filter(source, destination, |_, _| {
                Err(io::ErrorKind::CrossesDevices.into())
            })
            .unwrap();
            assert_eq!(fs::read(destination).unwrap(), fs::read(source).unwrap());
            fs::write(destination, b"independent copy").unwrap();
            assert_eq!(fs::read(source).unwrap(), b"synthetic executable image");
        });
    }

    #[test]
    fn filter_does_not_copy_after_other_link_errors() {
        fixture(|source, destination| {
            let error = link_shell_filter(source, destination, |_, _| {
                Err(io::ErrorKind::PermissionDenied.into())
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert!(!destination.exists());
        });
    }

    #[test]
    fn filter_copy_never_overwrites_existing_destination() {
        fixture(|source, destination| {
            fs::write(destination, b"preserve existing file").unwrap();
            let error = link_shell_filter(source, destination, |_, _| {
                Err(io::ErrorKind::CrossesDevices.into())
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(destination).unwrap(), b"preserve existing file");
        });
    }

    #[cfg(unix)]
    #[test]
    fn filter_copy_never_follows_existing_destination_symlink() {
        fixture(|source, destination| {
            std::os::unix::fs::symlink(source, destination).unwrap();
            let error = link_shell_filter(source, destination, |_, _| {
                Err(io::ErrorKind::CrossesDevices.into())
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(source).unwrap(), b"synthetic executable image");
        });
    }

    #[test]
    fn image_comparison_rejects_missing_or_different_image() {
        fixture(|source, destination| {
            let selected = source.canonicalize().unwrap();
            assert!(selected_image_matches(&selected, source));
            assert!(!selected_image_matches(&selected, destination));
            fs::write(destination, b"other image").unwrap();
            assert!(!selected_image_matches(&selected, destination));
        });
    }

    #[cfg(unix)]
    #[test]
    fn image_comparison_normalizes_filesystem_aliases() {
        fixture(|source, destination| {
            std::os::unix::fs::symlink(source, destination).unwrap();
            assert!(selected_image_matches(
                &source.canonicalize().unwrap(),
                destination
            ));
        });
    }

    #[cfg(windows)]
    #[test]
    fn windows_win32_and_verbatim_process_images_match() {
        let selected = std::env::current_exe().unwrap().canonicalize().unwrap();
        let native = crate::native::current().unwrap();
        assert!(selected_image_matches(&selected, &native.executable));
        let path = selected.to_str().unwrap();
        let win32 = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
        };
        assert!(selected_image_matches(&selected, Path::new(&win32)));
    }
}
