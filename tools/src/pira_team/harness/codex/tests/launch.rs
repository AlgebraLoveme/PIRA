use super::*;
use crate::{tests::options, write_notice};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn authentication_detach_survives_injected_diagnostic_write_failure() {
    struct BrokenOutput(usize);
    impl std::io::Write for BrokenOutput {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            self.0 += 1;
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }
    let root = std::env::temp_dir().join(format!(
        "team-detach-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    private_dir(&root).unwrap();
    // A directory makes unlink fail deterministically, without credentials or chmod.
    private_dir(&root.join("auth.json")).unwrap();
    let home = IsolatedHome(root.clone());
    let mut output = BrokenOutput(0);
    home.detach(|message| write_notice(&mut output, message));
    assert!(output.0 > 0);
    assert!(root.join("auth.json").is_dir());
    fs::remove_dir(root.join("auth.json")).unwrap();
    let mut warnings = 0;
    home.detach(|_| warnings += 1);
    assert_eq!(warnings, 0, "missing authentication is not a warning");
    fs::remove_dir(root).unwrap();
}

#[test]
fn worker_launcher_setting_does_not_redirect_backend_process() {
    let backend = Path::new("/ambient/bin/selected-codex");
    let mut command = Command::new(backend);
    command.env("UNCHANGED_TOOL_SETTING", "fixture");
    set_worker_launcher(&mut command).unwrap();
    assert_eq!(command.get_program(), backend.as_os_str());
    assert!(!command.get_envs().any(|(name, _)| name == "PATH"));
    assert!(
        command
            .get_envs()
            .any(|(name, value)| name == "UNCHANGED_TOOL_SETTING"
                && value == Some(std::ffi::OsStr::new("fixture")))
    );
    let setting = command
        .get_args()
        .find_map(|arg| {
            arg.to_str()?
                .strip_prefix("shell_environment_policy.set.PATH=")
        })
        .unwrap();
    let path: String = serde_json::from_str(setting).unwrap();
    assert_eq!(
        std::env::split_paths(&path).next().unwrap(),
        std::env::current_exe()
            .unwrap()
            .canonicalize()
            .unwrap()
            .parent()
            .unwrap()
    );
}

#[cfg(unix)]
#[test]
fn worker_path_executes_staged_launcher_instead_of_different_ambient_copy() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "pira-team-launcher-path-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    private_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let staged = root.join("staged parent");
    let ambient = root.join("ambient launcher");
    for directory in [&staged, &ambient] {
        private_dir(directory).unwrap();
        let program = directory.join("pira_team");
        fs::write(&program, "#!/bin/sh\nprintf '%s' \"$0\"\n").unwrap();
        fs::set_permissions(program, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let selected = |path: &std::ffi::OsStr| {
        let output = Command::new("pira_team")
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(output.status.success());
        PathBuf::from(String::from_utf8(output.stdout).unwrap())
            .canonicalize()
            .unwrap()
    };
    let inherited = std::env::join_paths([&ambient]).unwrap();
    assert_eq!(selected(&inherited), ambient.join("pira_team"));
    let path = worker_path(&staged.join("pira_team"), Some(&inherited)).unwrap();
    assert_eq!(
        selected(std::ffi::OsStr::new(&path)),
        staged.join("pira_team")
    );
}

#[test]
fn command_keeps_session_home_separate_from_revision_logs() {
    let opt = options(&["review"]).unwrap();
    let run = Path::new("/tmp/worker run");
    for dir in [run.to_owned(), run.join("revisions/000002/repair")] {
        let cmd = command(&opt, run, &dir, &dir.join("artifacts/handoff"));
        let home = cmd
            .get_envs()
            .find(|(key, _)| *key == "CODEX_HOME")
            .unwrap()
            .1
            .unwrap();
        assert_eq!(home, run.join("codex-home").as_os_str());
        for key in ["TMPDIR", "TMP", "TEMP", "TMPPREFIX"] {
            let path = if key == "TMPPREFIX" {
                opt.temp_workspace.as_ref().unwrap().join("zsh")
            } else {
                opt.temp_workspace.clone().unwrap()
            };
            let actual = cmd
                .get_envs()
                .find(|(name, _)| *name == key)
                .and_then(|(_, value)| value);
            assert_eq!(actual, Some(path.as_os_str()), "backend environment {key}");
            let setting = format!("shell_environment_policy.set.{key}={}", json!(path));
            assert!(cmd.get_args().any(|arg| arg == setting.as_str()));
        }
        assert_eq!(cmd.get_current_dir(), Some(dir.as_path()));
        let policy = format!("model_instructions_file={}", json!(dir.join("policy.md")));
        assert!(cmd.get_args().any(|arg| arg == policy.as_str()));
    }
}

#[test]
fn command_projects_worker_context_only_into_shell_settings() {
    let mut opt = options(&["review"]).unwrap();
    let run = Path::new("/tmp/worker \"run\" ü");
    opt.ctx_store = run.join("context store");
    opt.dec_store = run.join("decision store");
    for workspace in [None, Some(run.join("workspace anchor"))] {
        opt.dec_workspace = workspace;
        for dir in [run.to_owned(), run.join("revisions/000002/repair")] {
            let handoff = dir.join("artifacts/handoff.md");
            let cmd = command(&opt, run, &dir, &handoff);
            for (key, path) in [
                ("PIRA_CTX_STORE_DIR", Some(opt.ctx_store.as_path())),
                ("PIRA_DEC_STORE_DIR", Some(opt.dec_store.as_path())),
                ("PIRA_DEC_WORKSPACE_DIR", opt.dec_workspace.as_deref()),
                ("PIRA_TEAM_CONTEXT", Some(run)),
                ("PIRA_TEAM_ATTEMPT", Some(dir.as_path())),
                ("PIRA_TEAM_HANDOFF", Some(handoff.as_path())),
            ] {
                assert!(!cmd.get_envs().any(|(name, _)| name == key), "{key}");
                let prefix = format!("shell_environment_policy.set.{key}=");
                let actual: Vec<_> = cmd
                    .get_args()
                    .filter_map(|arg| arg.to_str().unwrap().strip_prefix(&prefix))
                    .collect();
                let expected: Vec<_> = path
                    .map(|path| json!(path).to_string())
                    .into_iter()
                    .collect();
                assert_eq!(actual, expected, "{key}");
            }
        }
    }
}

#[test]
fn launch_always_isolates_instructions_and_permissions() {
    let opt = options(&["review"]).unwrap();
    let cmd = command(
        &opt,
        Path::new("/tmp/worker run"),
        Path::new("/tmp/policy run"),
        Path::new("/tmp/policy run/artifacts/handoff"),
    );
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    for expected in [
        "app-server",
        "--strict-config",
        "sandbox_mode=\"workspace-write\"",
        "project_doc_max_bytes=0",
        "approval_policy=\"never\"",
        "agents.enabled=false",
        "features.multi_agent=false",
        "shell_environment_policy.set.PIRA_TEAM_CHILD=\"1\"",
    ] {
        assert!(args.iter().any(|a| a == expected), "{expected}");
    }
    assert!(
        cmd.get_envs().any(
            |(key, value)| key == "PIRA_TEAM_CHILD" && value == Some(std::ffi::OsStr::new("1"))
        )
    );
    assert!(
        !args
            .iter()
            .any(|a| a.contains("danger-full-access") || a == "resume")
    );
}
