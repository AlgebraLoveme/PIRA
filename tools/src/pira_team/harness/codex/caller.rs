//! Native Codex caller discovery and bounded current-turn transcript decoding.
use super::caller_log;
use crate::profile::{MAX_RECORD_BYTES, read_identity_file};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub(crate) fn caller_context() -> Result<Value, String> {
    let (path, thread) = codex_log()?;
    read(&path, &thread)
}

pub(crate) fn caller_identity() -> Result<String, String> {
    let (path, thread) = codex_log()?;
    let record = read_identity_file(caller_log::open(&path)?, |record| {
        (record["type"] == "session_meta").then_some(&record["payload"]["id"])
    })?;
    if record != thread {
        return Err("parent session identity mismatch".into());
    }
    Ok(thread)
}

fn codex_log() -> Result<(PathBuf, String), String> {
    let thread = std::env::var("CODEX_THREAD_ID").map_err(|_| "CODEX_THREAD_ID is unavailable")?;
    if thread.is_empty()
        || thread.len() > 200
        || !thread
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("invalid CODEX_THREAD_ID".into());
    }
    let worker = crate::subteam::caller_run()?;
    let home = worker
        .as_ref()
        .map(|run| run.join("codex-home").into_os_string())
        .or_else(|| std::env::var_os("CODEX_HOME"))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            ["HOME", "USERPROFILE"]
                .iter()
                .find_map(|key| std::env::var_os(key).filter(|value| !value.is_empty()))
                .map(|p| PathBuf::from(p).join(".codex"))
        })
        .ok_or("cannot locate Codex home")?;
    let home = if home.is_absolute() {
        home
    } else {
        std::env::current_dir()
            .map_err(|_| "cannot resolve Codex home")?
            .join(home)
    };
    let home = home
        .canonicalize()
        .map_err(|_| "cannot resolve selected Codex home")?;
    // Catch an accidentally selected worker home without its required launch context.
    if worker.is_none()
        && let Ok(physical) = home.canonicalize()
        && physical.file_name().and_then(|name| name.to_str()) == Some("codex-home")
        && let Some(run) = physical.parent()
        && run.join("run.lock").is_file()
        && run.join("manifest.json").is_file()
    {
        let manifest = crate::lifecycle::read_json(&run.join("manifest.json"))?;
        if manifest["transport"] == "app-server" && manifest["thread_id"] == thread {
            return Err(
                "Team worker session identity requires its complete launch context; resume with current Team"
                    .into(),
            );
        }
    }
    let index_home = if worker.is_none() {
        std::env::var_os("CODEX_SQLITE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    } else {
        None
    };
    let index_home = index_home.as_deref().unwrap_or(&home);
    let index_home = if index_home.is_absolute() {
        index_home.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| "cannot resolve Codex index home")?
            .join(index_home)
    };
    let index_home = index_home.canonicalize().unwrap_or(index_home);
    let indexed = caller_log::indexed(&index_home, &thread)?;
    let mut matches = Vec::new();
    if let Some(path) = indexed {
        matches.push(path);
    } else {
        // Resolve the selected session root once; discovery below never follows
        // links in arbitrary subtrees or matching transcript files.
        let sessions = home
            .join("sessions")
            .canonicalize()
            .map_err(|_| "cannot resolve native session root")?;
        collect(
            &sessions,
            &format!("-{thread}.jsonl"),
            0,
            &mut 0,
            &mut matches,
        )?;
    }
    if matches.len() != 1 {
        return Err("expected one matching parent session log".into());
    }
    if let Some(run) = worker {
        let manifest = crate::lifecycle::read_json(&run.join("manifest.json"))?;
        if manifest["thread_id"] != thread {
            return Err("caller does not match retained native worker identity".into());
        }
    }
    Ok((matches.remove(0), thread))
}

fn collect(
    dir: &Path,
    suffix: &str,
    depth: usize,
    visited: &mut usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), String> {
    // PIRA: bounded native YYYY/MM/DD fallback only; other layouts require the
    // native index. Never inspect unrelated subtrees or conversation contents.
    caller_log::directory(dir)?;
    for entry in fs::read_dir(dir).map_err(|_| "cannot read session directory")? {
        let entry = entry.map_err(|_| "cannot read session entry")?;
        *visited += 1;
        if *visited > 200_000 {
            return Err("session tree exceeds entry limit".into());
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let directory = match depth {
            0 => name.len() == 4 && name.bytes().all(|b| b.is_ascii_digit()),
            1 => name.len() == 2 && name.parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)),
            2 => name.len() == 2 && name.parse::<u8>().is_ok_and(|n| (1..=31).contains(&n)),
            _ => false,
        };
        let log = name.starts_with("rollout-") && name.ends_with(suffix);
        if !directory && !log {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|_| "cannot inspect matching session entry")?;
        if kind.is_dir() && directory {
            collect(&entry.path(), suffix, depth + 1, visited, matches)?;
        } else if kind.is_file() && log {
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
    let file = caller_log::open(path)?;
    let len = file
        .metadata()
        .map_err(|_| "cannot inspect parent session log")?
        .len();
    read_snapshot(file, len, thread)
}

fn read_snapshot(file: File, len: u64, thread: &str) -> Result<Value, String> {
    // A finite snapshot prevents an actively growing session from extending this read forever.
    // Bound memory per record, not by the total size of a long-running session.
    let mut reader = BufReader::new(file.take(len));
    let mut latest = None;
    let mut matched = false;
    // No boundary yet differs from a legacy task boundary without a turn ID.
    let mut active_turn: Option<Option<String>> = None;
    let mut line = Vec::new();
    loop {
        line.clear();
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
            }
            Some("event_msg") if record["payload"]["type"] == "task_started" => {
                latest = None;
                active_turn = Some(recorded_turn_id(&record["payload"])?.map(str::to_owned));
            }
            Some("turn_context") if matched => {
                let turn_id = recorded_turn_id(&record["payload"])?;
                if let Some(expected) = &active_turn
                    && expected.as_deref() != turn_id
                {
                    return Err("parent turn context does not match active task identity".into());
                }
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

fn recorded_turn_id(payload: &Value) -> Result<Option<&str>, String> {
    match &payload["turn_id"] {
        Value::Null => Ok(None),
        Value::String(id) if !id.is_empty() => Ok(Some(id)),
        _ => Err("malformed parent turn identity".into()),
    }
}

#[cfg(test)]
#[path = "tests/caller.rs"]
mod tests;
