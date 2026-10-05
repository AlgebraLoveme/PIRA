use serde_json::Value;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

pub fn default_root() -> PathBuf {
    if let Some(root) = std::env::var_os("PIRA_TEAM_DIR").filter(|s| !s.is_empty()) {
        return root.into();
    }
    #[cfg(unix)]
    let name = format!("pira-team-{}", unsafe { libc::geteuid() });
    #[cfg(not(unix))]
    let name = "pira-team";
    std::env::temp_dir().join(name)
}

pub fn access(args: &[String]) -> Result<(), String> {
    let mut root = default_root();
    let mut positional = Vec::new();
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == "--store" {
            root = rest.next().ok_or("missing --store directory")?.into();
        } else if arg.starts_with('-') {
            return Err(format!("unknown lookup option {arg}"));
        } else {
            positional.push(arg.as_str());
        }
    }
    if !(1..=2).contains(&positional.len()) {
        return Err("expected read|path RUN_ID [RELATIVE_FILE] [--store DIR]".into());
    }
    let run = locate(&root, positional[0])?;
    let relative = match positional.get(1) {
        Some(path) => PathBuf::from(path),
        None => artifact_path(&run)?,
    };
    let path = if relative == Path::new(".") && args[0] == "path" {
        run.clone()
    } else {
        managed_path(&run, &relative)?
    };
    if args[0] == "path" {
        println!("{}", path.display());
    } else {
        if !path.is_file() {
            return Err("read requires a regular file".into());
        }
        let mut file = File::open(&path).map_err(|e| format!("open managed file: {e}"))?;
        io::copy(&mut file, &mut io::stdout().lock())
            .map_err(|e| format!("read managed file: {e}"))?;
    }
    Ok(())
}

pub fn locate(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("invalid run ID; use run_id from the receipt".into());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("open Team store: {e}"))?;
    let run = root.join(id);
    let meta = fs::symlink_metadata(&run).map_err(|e| format!("open run {id}: {e}"))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("run must be a real directory".into());
    }
    Ok(run)
}

fn artifact_path(run: &Path) -> Result<PathBuf, String> {
    let path = managed_path(run, Path::new("manifest.json"))?;
    let file = File::open(path).map_err(|e| format!("open run manifest: {e}"))?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("run manifest exceeds 16 MiB".into());
    }
    let manifest: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid run manifest")?;
    if !["completed", "needs_decision", "incomplete"]
        .contains(&manifest["status"].as_str().unwrap_or(""))
    {
        return Err(
            "run has no completed or decision outcome; inspect manifest.json or diagnostic files explicitly".into(),
        );
    }
    let relative = if let Some(path) = manifest["artifact"].as_str() {
        PathBuf::from(path)
    } else {
        // Compatibility with completed runs written before relative artifact metadata.
        let result = Path::new(
            manifest["result"]
                .as_str()
                .ok_or("manifest has no artifact")?,
        );
        result
            .strip_prefix(run)
            .map_err(|_| "manifest artifact escapes its run")?
            .to_owned()
    };
    let parts: Vec<_> = relative.iter().filter_map(|p| p.to_str()).collect();
    if !matches!(
        parts.as_slice(),
        ["artifacts", _] | ["revisions", _, "artifacts", _]
    ) {
        return Err("manifest artifact is outside artifacts/".into());
    }
    Ok(relative)
}

fn managed_path(run: &Path, relative: &Path) -> Result<PathBuf, String> {
    let parts: Vec<_> = relative.components().collect();
    if parts.is_empty() || parts.iter().any(|p| !matches!(p, Component::Normal(_))) {
        return Err("managed file must be a relative path without traversal".into());
    }
    // Only launcher-owned outputs are exposed, never the live isolated home/authentication.
    let logs = [
        "manifest.json",
        "policy.md",
        "phase.md",
        "task.txt",
        "events.jsonl",
        "stderr.log",
        "candidate.txt",
        "rejected-handoff.txt",
        "validation.json",
        "requests.jsonl",
        "controls.jsonl",
    ];
    let names: Vec<_> = parts
        .iter()
        .map(|p| p.as_os_str().to_str().unwrap_or(""))
        .collect();
    let local = match names.as_slice() {
        ["revisions", revision, rest @ ..]
            if revision.len() == 6 && revision.bytes().all(|b| b.is_ascii_digit()) =>
        {
            rest
        }
        _ => names.as_slice(),
    };
    let allowed = match local {
        [name] => logs.contains(name) || *name == "artifacts" || *name == "repair",
        ["artifacts", _] => true,
        ["repair", name] => logs.contains(name),
        _ => false,
    };
    if !allowed {
        return Err("not a managed artifact or diagnostic path".into());
    }
    let mut path = run.to_owned();
    for part in parts {
        path.push(part);
        let metadata =
            fs::symlink_metadata(&path).map_err(|e| format!("inspect managed file: {e}"))?;
        if metadata.file_type().is_symlink() || !(metadata.is_dir() || metadata.is_file()) {
            return Err("managed paths must not contain symlinks or special files".into());
        }
    }
    Ok(path)
}
