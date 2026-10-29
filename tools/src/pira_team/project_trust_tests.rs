#[test]
fn command_encodes_project_trust_as_a_toml_value() {
    for (cwd, quoted_key) in [
        ("/tmp/plain", r#""/tmp/plain""#),
        ("/tmp/a.b", r#""/tmp/a.b""#),
        (r#"C:\Users\a.b\project"#, r#""C:\\Users\\a.b\\project""#),
        (
            "/tmp/雪🚀\"\\=\n\t\r\u{8}\u{c}\u{1}\u{1f}\u{7f}\u{85}",
            "\"/tmp/雪🚀\\\"\\\\=\\n\\t\\r\\b\\f\\u0001\\u001f\\u007f\u{85}\"",
        ),
    ] {
        let mut opt = options(&["probe"]).unwrap();
        opt.cwd = PathBuf::from(cwd);
        let cmd = command(
            &opt,
            Path::new("/tmp/run"),
            Path::new("/tmp/run"),
            Path::new("/tmp/handoff"),
        );
        let settings: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy())
            .filter(|a| a.starts_with("projects"))
            .collect();
        assert_eq!(
            settings,
            [format!(
                "projects={{{quoted_key}={{trust_level=\"untrusted\"}}}}"
            )]
        );
    }
}

// Probe functionality, not PATH presence: Windows Store aliases can resolve but fail.
fn project_trust_python_with(
    mut usable: impl FnMut(&mut Command) -> Result<bool, String>,
) -> Result<Command, String> {
    let mut failures = Vec::new();
    for (program, prefix) in [
        ("python3", &[][..]),
        ("python", &[][..]),
        ("py", &["-3"][..]),
    ] {
        let mut probe = Command::new(program);
        probe.env_clear().stdin(std::process::Stdio::null());
        for name in ["PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                probe.env(name, value);
            }
        }
        probe.args(prefix).args([
            "-I",
            "-c",
            "import sys; assert sys.version_info >= (3, 11); print('pira-team-python-ok')",
        ]);
        match usable(&mut probe) {
            Ok(true) => {
                let mut command = Command::new(program);
                command.args(prefix);
                return Ok(command);
            }
            Ok(false) => failures.push(format!("{program}: not functional Python 3.11+")),
            Err(error) => failures.push(format!("{program}: {error}")),
        }
    }
    Err(format!(
        "native project-trust probe requires working Python 3.11+ on PATH (python3, python, or py -3): {}",
        failures.join("; ")
    ))
}

fn project_trust_python() -> Result<Command, String> {
    project_trust_python_with(|probe| {
        let output = probe.output().map_err(|error| error.to_string())?;
        Ok(output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == "pira-team-python-ok")
    })
}

#[test]
fn project_trust_python_falls_back_from_store_alias_to_python() {
    let mut attempted = Vec::new();
    let command = project_trust_python_with(|probe| {
        let program = probe.get_program().to_string_lossy().into_owned();
        attempted.push(program.clone());
        assert!(probe.get_args().any(|arg| arg == "-I"));
        assert!(probe.get_envs().all(|(key, _)| {
            ["PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"]
                .iter()
                .any(|allowed| key == *allowed)
        }));
        Ok(program == "python")
    })
    .unwrap();
    assert_eq!(attempted, ["python3", "python"]);
    assert_eq!(command.get_program(), "python");
    assert_eq!(command.get_args().count(), 0);
}

#[test]
fn project_trust_python_uses_py_launcher_and_reports_exhaustion() {
    let mut attempted = Vec::new();
    let command = project_trust_python_with(|probe| {
        let program = probe.get_program().to_string_lossy().into_owned();
        attempted.push(program.clone());
        if program == "python3" {
            Err("not found".into())
        } else {
            Ok(program == "py")
        }
    })
    .unwrap();
    assert_eq!(attempted, ["python3", "python", "py"]);
    assert_eq!(command.get_program(), "py");
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["-3"]);
    let error = project_trust_python_with(|_| Ok(false)).unwrap_err();
    assert!(error.contains("Python 3.11+"));
    for program in ["python3:", "python:", "py:"] {
        assert!(error.contains(program));
    }
}

#[test]
#[ignore = "requires installed native Codex and Python 3.11+; no credentials or model calls"]
fn installed_native_launcher_preserves_exact_untrusted_project_keys() {
    let root = std::env::temp_dir().join(format!(
        "team-project-trust-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let result = (|| -> Result<(), String> {
        let names = if cfg!(windows) {
            vec!["plain", "a.b", "space = 雪🚀"]
        } else {
            vec![
                "plain",
                "a.b",
                "space. 雪🚀\"\\=\n\t\u{1}\u{1f}\u{7f}\u{85}",
            ]
        };
        for (index, name) in names.iter().enumerate() {
            let run = root.join(format!("run-{index}"));
            let home = run.join("codex-home");
            fs::create_dir_all(&home).map_err(|e| e.to_string())?;
            fs::create_dir(run.join("scratch")).map_err(|e| e.to_string())?;
            fs::write(run.join("policy.md"), "Isolated configuration probe.")
                .map_err(|e| e.to_string())?;
            let project = root.join(name);
            fs::create_dir_all(project.join(".git")).map_err(|e| e.to_string())?;
            fs::create_dir(project.join(".codex")).map_err(|e| e.to_string())?;
            fs::write(
                project.join(".codex/config.toml"),
                "model=\"project-layer-must-not-load\"\n",
            )
            .map_err(|e| e.to_string())?;
            let project = fs::canonicalize(project).map_err(|e| e.to_string())?;
            let key = json!(project).to_string().replace('\u{7f}', "\\u007f");
            fs::write(
                home.join("config.toml"),
                format!("[projects.{key}]\ntrust_level=\"trusted\"\n"),
            )
            .map_err(|e| e.to_string())?;
            let mut opt = options(&["probe"]).map_err(|e| e.to_string())?;
            opt.cwd = project.clone();
            let cmd = command(&opt, &run, &run, &run.join("handoff"));
            let argv: Vec<_> = std::iter::once(cmd.get_program())
                .chain(cmd.get_args())
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let mut probe = project_trust_python()?;
            probe.env_clear();
            for name in ["PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"] {
                if let Some(value) = std::env::var_os(name) {
                    probe.env(name, value);
                }
            }
            probe
                .env("HOME", &home)
                .env("USERPROFILE", &home)
                .env("PYTHONDONTWRITEBYTECODE", "1");
            for (key, value) in cmd.get_envs() {
                if let Some(value) = value {
                    probe.env(key, value);
                }
            }
            let output = probe.current_dir(cmd.get_current_dir().unwrap()).args(["-c", r#"
import json, os, sys
sys.path.insert(0, sys.argv[3])
import setup_pira_tools
setup_pira_tools.check_team_initialize(json.loads(sys.argv[1]), dict(os.environ), project=sys.argv[2])
"#]).arg(json!(argv).to_string()).arg(&project)
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/scripts"))
                .output().map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "project {project:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        Ok(())
    })();
    fs::remove_dir_all(root).unwrap();
    result.unwrap();
}
