use serde_json::Value;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

pub fn default_root() -> Result<PathBuf, String> {
    configured_root("PIRA_TEAM_DIR", "team")
}

pub fn configured_root(key: &str, leaf: &str) -> Result<PathBuf, String> {
    resolve_root(std::env::consts::OS, key, leaf, |name| {
        std::env::var_os(name)
    })
}

fn resolve_root(
    os: &str,
    key: &str,
    leaf: &str,
    get: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, String> {
    let value = |name| get(name).filter(|s| !s.is_empty()).map(PathBuf::from);
    if let Some(root) = value(key) {
        return Ok(root);
    }
    let parent = match os {
        "macos" => {
            value("HOME").map(|p| p.join("Library").join("Application Support").join("PIRA"))
        }
        "windows" => value("LOCALAPPDATA").map(|p| p.join("PIRA")),
        _ => value("XDG_DATA_HOME")
            // XDG uses Unix absolute-path syntax, independent of the test host OS.
            .filter(|p| p.as_os_str().as_encoded_bytes().starts_with(b"/"))
            .or_else(|| value("HOME").map(|p| p.join(".local").join("share")))
            .map(|p| p.join("pira")),
    };
    parent.map(|p| p.join(leaf)).ok_or_else(|| {
        format!(
            "cannot resolve persistent PIRA store; set {key} explicitly or configure {}",
            match os {
                "windows" => "LOCALAPPDATA",
                "macos" => "HOME",
                _ => "XDG_DATA_HOME or HOME",
            }
        )
    })
}

pub fn access(args: &[String]) -> Result<(), String> {
    let mut root = None;
    let mut positional = Vec::new();
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == "--store" {
            root = Some(PathBuf::from(
                rest.next().ok_or("missing --store directory")?,
            ));
        } else if arg.starts_with('-') {
            return Err(format!("unknown lookup option {arg}"));
        } else {
            positional.push(arg.as_str());
        }
    }
    if !(1..=2).contains(&positional.len()) {
        return Err("expected read|path RUN_ID [RELATIVE_FILE] [--store DIR]".into());
    }
    let root = root.map(Ok).unwrap_or_else(default_root)?;
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

pub(crate) fn validate_run_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("invalid run ID; use run_id from the receipt".into());
    }
    Ok(())
}

pub fn locate(root: &Path, reference: &str) -> Result<PathBuf, String> {
    // Literal IDs keep their existing lookup path, even when bookmark metadata is bad.
    let resolved;
    let id = if let Some(label) = reference.strip_prefix('@') {
        resolved = crate::bookmarks::resolve(root, label)?;
        resolved.as_str()
    } else {
        reference
    };
    validate_run_id(id)?;
    let root = root
        .canonicalize()
        .map_err(|e| lookup_error("open Team store", e))?;
    let run = root.join(id);
    let meta =
        fs::symlink_metadata(&run).map_err(|e| lookup_error(&format!("open run {id}"), e))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("run must be a real directory".into());
    }
    Ok(run)
}

fn lookup_error(context: &str, error: io::Error) -> String {
    let mut message = format!("{context}: {error}");
    if error.kind() == io::ErrorKind::NotFound {
        message.push_str("; Team now defaults to the persistent PIRA/team store. For an older temporary-store run, pass --store OLD_STORE (or PIRA_TEAM_DIR). The former default was the platform temp directory's pira-team-UID on Unix, pira-team on Windows. Use the dedicated store migration tooling for relocation: retained runs contain absolute paths; do not simply move/merge directories");
    }
    message
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
        ["artifacts", _]
            | ["implementation", "artifacts", _]
            | ["revisions", _, "artifacts", _]
            | ["revisions", _, "implementation", "artifacts", _]
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
        "backend-check.log",
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
    let local = local.strip_prefix(&["implementation"]).unwrap_or(local);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_defaults_share_the_platform_data_parent() {
        for (os, settings, parent) in [
            (
                "macos",
                vec![("HOME", "/home/person"), ("XDG_DATA_HOME", "/ignored")],
                "/home/person/Library/Application Support/PIRA",
            ),
            (
                "linux",
                vec![("HOME", "/home/person"), ("XDG_DATA_HOME", "/data")],
                "/data/pira",
            ),
            (
                "linux",
                vec![("HOME", "/home/person")],
                "/home/person/.local/share/pira",
            ),
            (
                "linux",
                vec![("HOME", "/home/person"), ("XDG_DATA_HOME", "")],
                "/home/person/.local/share/pira",
            ),
            (
                "linux",
                vec![("HOME", "/home/person"), ("XDG_DATA_HOME", "relative")],
                "/home/person/.local/share/pira",
            ),
            ("linux", vec![("XDG_DATA_HOME", "/data")], "/data/pira"),
            (
                "windows",
                vec![
                    ("LOCALAPPDATA", "C:/Users/person/AppData/Local"),
                    ("HOME", "/ignored"),
                ],
                "C:/Users/person/AppData/Local/PIRA",
            ),
        ] {
            for (key, leaf) in [
                ("PIRA_TEAM_DIR", "team"),
                ("PIRA_CTX_STORE_DIR", "ctx"),
                ("PIRA_DEC_STORE_DIR", "decision"),
            ] {
                let resolved = resolve_root(os, key, leaf, |name| {
                    settings
                        .iter()
                        .find(|(k, _)| *k == name)
                        .map(|(_, v)| OsString::from(v))
                })
                .unwrap();
                assert_eq!(resolved, Path::new(parent).join(leaf), "{os} {leaf}");
            }
        }
    }

    #[test]
    fn explicit_stores_do_not_require_home_and_missing_defaults_fail_visibly() {
        for os in ["macos", "linux", "windows"] {
            for (key, leaf) in [
                ("PIRA_TEAM_DIR", "team"),
                ("PIRA_CTX_STORE_DIR", "ctx"),
                ("PIRA_DEC_STORE_DIR", "decision"),
            ] {
                assert_eq!(
                    resolve_root(os, key, leaf, |name| (name == key)
                        .then(|| "custom store".into()))
                    .unwrap(),
                    Path::new("custom store")
                );
                let error = resolve_root(os, key, leaf, |_| None).unwrap_err();
                assert!(error.contains(key));
                assert!(resolve_root(os, key, leaf, |_| Some("".into())).is_err());
            }
        }
    }
}
