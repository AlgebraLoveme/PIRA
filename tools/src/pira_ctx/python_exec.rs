use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::Config;
use crate::model::StreamKind;
use crate::storage::StoredResult;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

const BOOTSTRAP: &str = r#"import json, sys
manifest_path, code_path, source_name = sys.argv[1:4]
with open(manifest_path, "rb") as _f:
    entries = json.load(_f)
CAPTURES = {}
for entry in entries:
    with open(entry["path"], "rb") as _f:
        exact = _f.read()
    CAPTURES[entry["name"]] = {
        "text": exact.decode("utf-8", "replace"),
        "bytes": exact,
        "path": entry["path"],
        "stdout_path": entry["stdout_path"],
        "stderr_path": entry["stderr_path"],
        "id": entry["id"],
        "exit": entry["exit"],
        "state": entry["state"],
        "generation": entry["generation"],
    }
CAPTURE_NAMES = list(CAPTURES)
MSGS = [CAPTURES[name]["text"] for name in CAPTURE_NAMES]
MSG_BYTES_LIST = [CAPTURES[name]["bytes"] for name in CAPTURE_NAMES]
MSG_IDS = [CAPTURES[name]["id"] for name in CAPTURE_NAMES]
with open(code_path, "rb") as _f:
    source = _f.read()
scope = {
    "__name__": "__main__",
    "__file__": source_name,
    "CAPTURES": CAPTURES,
    "CAPTURE_NAMES": CAPTURE_NAMES,
    "MSGS": MSGS,
    "MSG_BYTES_LIST": MSG_BYTES_LIST,
    "MSG_IDS": MSG_IDS,
}
if len(CAPTURE_NAMES) == 1:
    _single = CAPTURES[CAPTURE_NAMES[0]]
    scope.update({
        "MSG": _single["text"],
        "MSG_BYTES": _single["bytes"],
        "MSG_PATH": _single["path"],
        "MSG_STDOUT_PATH": _single["stdout_path"],
        "MSG_STDERR_PATH": _single["stderr_path"],
        "MSG_ID": _single["id"],
        "MSG_EXIT": _single["exit"],
        "MSG_STATE": _single["state"],
        "MSG_GENERATION": _single["generation"],
    })
sys.argv = [source_name]
exec(compile(source, source_name, "exec"), scope, scope)
"#;
const DEFAULT_MAX_EXEC_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ANALYSIS_CODE_BYTES: u64 = 1024 * 1024;

pub struct PreparedExec {
    _workspace: PrivateWorkspace,
    pub command: Vec<String>,
}

pub fn prepare(
    config: &Config,
    sources: &[(String, StoredResult)],
) -> Result<PreparedExec, String> {
    if sources.is_empty() {
        return Err("exec requires at least one resolved capture".into());
    }
    let maximum = std::env::var("PIRA_CTX_MAX_EXEC_BYTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(DEFAULT_MAX_EXEC_BYTES, |value| value.max(4 * 1024));
    let mut total_bytes = 0_u64;
    for (name, source) in sources {
        if source.metadata.timeline_truncated {
            return Err(format!(
                "cannot construct merged text for input {name:?} with a truncated line index"
            ));
        }
        total_bytes = total_bytes
            .checked_add(source.metadata.total_bytes)
            .ok_or("combined capture size overflow")?;
    }
    if total_bytes > maximum {
        return Err(format!(
            "combined captures are {total_bytes} bytes; exec materialization is limited to {maximum} bytes by PIRA_CTX_MAX_EXEC_BYTES; use search/transform or raise the limit deliberately"
        ));
    }
    let mut command = resolve_python(config)?;
    let workspace = PrivateWorkspace::create()?;
    let manifest_path = workspace.path.join("captures.json");
    let code_path = workspace.path.join("analysis.py");
    let mut manifest = Vec::with_capacity(sources.len());
    for (index, (name, source)) in sources.iter().enumerate() {
        let merged_path = workspace.path.join(format!("input-{index:02}-merged.log"));
        let stdout_path = workspace.path.join(format!("input-{index:02}-stdout.log"));
        let stderr_path = workspace.path.join(format!("input-{index:02}-stderr.log"));
        materialize(source, &merged_path, &stdout_path, &stderr_path)?;
        manifest.push(serde_json::json!({
            "name": name,
            "path": merged_path.to_string_lossy(),
            "stdout_path": stdout_path.to_string_lossy(),
            "stderr_path": stderr_path.to_string_lossy(),
            "id": source.metadata.result_id,
            "exit": if source.is_running() { serde_json::Value::Null } else { serde_json::json!(source.metadata.exit_code) },
            "state": if source.is_running() { "running" } else { "complete" },
            "generation": source.live_generation().unwrap_or_default(),
        }));
    }
    let manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|error| format!("cannot encode exec input manifest: {error}"))?;
    write_private(&manifest_path, &manifest_bytes)?;

    let (code, source_name) = match (&config.exec_code, &config.exec_file) {
        (Some(code), None) => (code.as_bytes().to_vec(), "<pira_ctx-exec>".to_string()),
        (None, Some(path)) if path == Path::new("-") => {
            (read_stdin_limited()?, "<stdin>".to_string())
        }
        (None, Some(path)) => (
            crate::util::read_file_limited(path, MAX_ANALYSIS_CODE_BYTES, "analysis file")?,
            path.display().to_string(),
        ),
        _ => return Err("choose exactly one --code CODE or --file PATH".into()),
    };
    if code.len() as u64 > MAX_ANALYSIS_CODE_BYTES {
        return Err(format!(
            "analysis code exceeds the {MAX_ANALYSIS_CODE_BYTES}-byte limit"
        ));
    }
    write_private(&code_path, &code)?;

    command.extend([
        "-c".to_string(),
        BOOTSTRAP.to_string(),
        manifest_path.display().to_string(),
        code_path.display().to_string(),
        source_name,
    ]);
    Ok(PreparedExec {
        _workspace: workspace,
        command,
    })
}

fn read_stdin_limited() -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_ANALYSIS_CODE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read analysis code from stdin: {error}"))?;
    if bytes.len() as u64 > MAX_ANALYSIS_CODE_BYTES {
        return Err(format!(
            "analysis code from stdin exceeds the {MAX_ANALYSIS_CODE_BYTES}-byte limit"
        ));
    }
    Ok(bytes)
}

fn materialize(
    source: &StoredResult,
    merged_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<(), String> {
    let mut reader = source.reader()?;
    let mut stdout = create_private(stdout_path)?;
    let mut stderr = create_private(stderr_path)?;
    reader.copy_section(StreamKind::Stdout, &mut stdout)?;
    reader.copy_section(StreamKind::Stderr, &mut stderr)?;

    let mut reader = source.reader()?;
    let mut merged = create_private(merged_path)?;
    for line in &source.metadata.line_timeline {
        reader.copy_line(line, &mut merged)?;
    }
    Ok(())
}

fn resolve_python(config: &Config) -> Result<Vec<String>, String> {
    if let Some(program) = config.python.as_deref() {
        let candidate = vec![program.to_string()];
        probe_python(&candidate).map_err(|error| format!("invalid --python PATH: {error}"))?;
        return Ok(candidate);
    }
    if let Some(program) = std::env::var_os("PIRA_CTX_PYTHON") {
        let candidate = vec![program.to_string_lossy().into_owned()];
        probe_python(&candidate).map_err(|error| format!("invalid PIRA_CTX_PYTHON: {error}"))?;
        return Ok(candidate);
    }

    let mut candidates = vec![vec!["python3".to_string()]];
    #[cfg(windows)]
    candidates.push(vec!["py".to_string(), "-3".to_string()]);
    candidates.push(vec!["python".to_string()]);
    for candidate in candidates {
        if probe_python(&candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Err("Python 3 was not found; install it, pass --python PATH, or set PIRA_CTX_PYTHON".into())
}

fn probe_python(candidate: &[String]) -> Result<(), String> {
    let status = Command::new(&candidate[0])
        .args(&candidate[1..])
        .args([
            "-c",
            "import sys; raise SystemExit(0 if sys.version_info.major == 3 else 1)",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("cannot run {}: {error}", candidate[0]))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} is not a working Python 3 interpreter",
            candidate[0]
        ))
    }
}

struct PrivateWorkspace {
    path: PathBuf,
}

impl PrivateWorkspace {
    fn create() -> Result<Self, String> {
        let base = std::env::temp_dir();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for nonce in 0..100_u32 {
            let path = base.join(format!(
                ".pira_ctx-exec-{}-{now}-{nonce}",
                std::process::id()
            ));
            #[cfg(unix)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(not(unix))]
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("create private analysis workspace: {error}")),
            }
        }
        Err("could not create a unique private analysis workspace".into())
    }
}

impl Drop for PrivateWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn create_private(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
        .open(path)
        .map_err(|error| format!("create private analysis file: {error}"))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = create_private(path)?;
    file.write_all(bytes)
        .map_err(|error| format!("write private analysis file: {error}"))
}
