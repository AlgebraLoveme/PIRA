// Adapted from the archived Team profile reader: inherit settings, never parent messages.
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SESSION_BYTES: u64 = 256 * 1024 * 1024;

pub fn resolve(
    model: Option<String>,
    effort: Option<String>,
) -> Result<(String, String, Value), String> {
    if let Some(m) = &model {
        validate_model(m)?;
    }
    if let Some(e) = &effort {
        validate_effort(e)?;
    }
    let sources = json!({"model": if model.is_some() { "explicit" } else { "parent" },
        "effort": if effort.is_some() { "explicit" } else { "parent" }});
    let parent = if model.is_none() || effort.is_none() {
        Some(load().map_err(|e| {
            format!("cannot inherit parent profile: {e}; supply --model and --effort explicitly")
        })?)
    } else {
        None
    };
    let inherited = |name: &str| {
        parent
            .as_ref()
            .and_then(|p| p.get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("parent {name} is unavailable; supply --{name} explicitly"))
    };
    let model = match model {
        Some(m) => m,
        None => inherited("model")?,
    };
    let effort = match effort {
        Some(e) => e,
        None => inherited("effort")?,
    };
    validate_model(&model)?;
    validate_effort(&effort)?;
    Ok((model, effort, sources))
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

fn load() -> Result<Value, String> {
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
                .find_map(std::env::var_os)
                .filter(|s| !s.is_empty())
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
                // Replace both fields atomically: never backfill a missing field from an older turn.
                latest = Some(
                    json!({"model": record["payload"]["model"], "effort": record["payload"]["effort"]}),
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
