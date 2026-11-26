//! Observational retained search: no backend discovery, worker startup, or log reads.
use crate::{bookmarks, lifecycle, profile, remote, storage};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const FIELDS: &[&str] = &[
    "run_id",
    "label",
    "revision",
    "revision_at",
    "status",
    "excerpt",
];
pub(super) const BATCH_SIZE: usize = 128;

pub(super) fn validate(selector: &str, pattern: &str, limit: usize) -> Result<(), String> {
    if !matches!(selector, "input" | "gate" | "final" | "label") {
        return Err("search selector must be input, gate, final or label".into());
    }
    if pattern.trim().is_empty() || pattern.len() > 128 || pattern.chars().any(char::is_control) {
        return Err(
            "pattern must be nonblank, without controls, and at most 128 UTF-8 bytes".into(),
        );
    }
    if !(1..=1000).contains(&limit) {
        return Err("--limit must be from 1 through 1000".into());
    }
    Ok(())
}

pub(super) fn label_for<'a>(
    labels: &'a [String],
    selector: &str,
    pattern: &str,
) -> Option<&'a str> {
    labels
        .iter()
        .find(|label| selector != "label" || label.to_lowercase().contains(&pattern.to_lowercase()))
        .map(String::as_str)
}

fn excerpt(text: &str, pattern: &str) -> Option<String> {
    let offset = text.to_lowercase().find(&pattern.to_lowercase())?;
    // Lowercasing can expand a character (e.g. İ); translate the match back to
    // original character positions before applying the 240-character window.
    let mut lower_bytes = 0;
    let position = text
        .chars()
        .position(|c| {
            lower_bytes += c.to_lowercase().map(char::len_utf8).sum::<usize>();
            offset < lower_bytes
        })
        .unwrap_or(0);
    Some(
        text.chars()
            .skip(position.saturating_sub(60))
            .take(240)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect(),
    )
}

fn revision_time(manifest: &Value) -> Option<String> {
    let ms = manifest["revision_started_at_ms"].as_u64()?;
    // Gregorian civil date from Unix days; no platform timezone or process required.
    let days = i64::try_from(ms / 86_400_000).ok()?;
    if days > 2_932_896 {
        return None;
    } // UTC years 1970..9999 only.
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let seconds = ms / 1000 % 86400;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        ms % 1000
    ))
}

fn handoff(run: &Path, manifest: &Value) -> Option<String> {
    let relative = storage::artifact_for(run, manifest).ok()?;
    // Only the accepted artifact of this revision, never a review checkpoint,
    // attachment, rejected candidate, or a newer turn's mutable metadata.
    let number = manifest["revision"].as_u64().unwrap_or(1);
    let prefix = if number == 1 {
        PathBuf::new()
    } else {
        PathBuf::from(format!("revisions/{number:06}"))
    };
    let local = relative.strip_prefix(prefix).ok()?;
    let local = local.strip_prefix("implementation").unwrap_or(local);
    if local.parent()? != Path::new("artifacts") || local.file_name()? == "review-checkpoint.md" {
        return None;
    }
    let path = storage::managed_path(run, &relative).ok()?;
    lifecycle::private_path(&path, false).ok()?;
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 8 * 1024 * 1024 {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn latest_handoff(run: &Path, current: &Value) -> Option<(Value, String)> {
    if let Some(text) = handoff(run, current) {
        return Some((current.clone(), text));
    }
    let mut numbers: Vec<_> = current["revisions"]
        .as_array()?
        .iter()
        .filter_map(|revision| revision["revision"].as_u64())
        .filter(|number| *number > 0 && *number <= current["revision"].as_u64().unwrap_or(0))
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    let creator = storage::creator_session(run).ok()?;
    for number in numbers.into_iter().rev() {
        let relative = PathBuf::from(format!("revisions/{number:06}/manifest.json"));
        let Some(manifest) = storage::managed_path(run, &relative)
            .ok()
            .and_then(|path| lifecycle::read_json(&path).ok())
        else {
            continue;
        };
        let snapshot_creator = match manifest.get("creator_main_session_id") {
            Some(owner) => owner.as_str().map(str::to_owned),
            None => storage::original_creator_session(run, current).ok(),
        };
        if manifest["run_id"] != current["run_id"]
            || snapshot_creator.as_deref() != Some(creator.as_str())
            || manifest["revision"] != number
        {
            continue;
        }
        if let Some(text) = handoff(run, &manifest) {
            return Some((manifest, text));
        }
    }
    None
}

/// Caller must first establish ownership using local locate_for or remote owned.
pub(super) fn hit(
    run: &Path,
    selector: &str,
    pattern: &str,
    label: Option<&str>,
) -> Result<Option<Value>, String> {
    let current = lifecycle::read_json(&run.join("manifest.json"))?;
    let (manifest, text) = if selector == "final" {
        let Some(delivered) = latest_handoff(run, &current) else {
            return Ok(None);
        };
        delivered
    } else {
        let key = if selector == "input" {
            "task"
        } else {
            "completion_gate"
        };
        let text = if selector == "label" {
            label.unwrap_or("")
        } else {
            current[key].as_str().unwrap_or("")
        }
        .to_owned();
        (current, text)
    };
    let Some(excerpt) = excerpt(&text, pattern) else {
        return Ok(None);
    };
    Ok(Some(
        json!({"run_id":run.file_name().and_then(|s| s.to_str()), "label":label,
        "revision":manifest["revision"], "revision_at":revision_time(&manifest),
        "status":manifest["status"], "excerpt":excerpt}),
    ))
}

pub(super) fn page(mut hits: Vec<Value>, limit: usize) -> Value {
    hits.sort_by(|a, b| {
        b["revision_at"]
            .as_str()
            .cmp(&a["revision_at"].as_str())
            .then_with(|| a["run_id"].as_str().cmp(&b["run_id"].as_str()))
    });
    let mut ids = BTreeSet::new();
    hits.retain(|hit| ids.insert(hit["run_id"].as_str().unwrap_or("").to_owned()));
    let has_more = hits.len() > limit;
    hits.truncate(limit);
    json!({"matches":hits,"has_more":has_more})
}

fn search(
    root: &Path,
    session: &str,
    selector: &str,
    pattern: &str,
    limit: usize,
) -> Result<Value, String> {
    let Some(root) = storage::managed::root_if_present(root)? else {
        return Ok(page(Vec::new(), limit));
    };
    let labels = bookmarks::labels_for(&root, session)?;
    let mut hits = Vec::new();
    let mut errors = Vec::new();
    for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if storage::validate_run_id(&id).is_err() {
            continue;
        }
        // Ordinary reads resolve retained routing first and reject collisions.
        if !matches!(
            remote::routing::reference_for(&root, &id, session),
            Ok(None)
        ) {
            continue;
        }
        let Ok(run) = storage::locate_for(&root, &id, session) else {
            continue;
        };
        let label = label_for(
            labels.get(&id).map(Vec::as_slice).unwrap_or_default(),
            selector,
            pattern,
        );
        if selector == "label" && label.is_none() {
            continue;
        }
        match hit(&run, selector, pattern, label) {
            Ok(Some(hit)) => hits.push(hit),
            Ok(None) => {}
            Err(_) => errors.push(json!({"run_id":id,"error":"retained content unavailable"})),
        }
    }
    let remote = remote::routing::search(&root, session, selector, pattern, limit, &labels)?;
    hits.extend(remote["matches"].as_array().unwrap().iter().cloned());
    errors.extend(remote["errors"].as_array().unwrap().iter().cloned());
    let mut result = page(hits, limit);
    if remote["has_more"] == true {
        result["has_more"] = json!(true);
    }
    if !errors.is_empty() {
        result["incomplete"] = json!(true);
        result["errors"] = json!(errors);
    }
    Ok(result)
}

pub(super) fn command(args: &[String]) -> Result<(), String> {
    let (mut store, mut limit, mut fields) = (None, None, None);
    let mut positional = Vec::new();
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            positional.extend(rest.map(String::as_str));
            break;
        }
        if !arg.starts_with('-') {
            positional.push(arg.as_str());
            continue;
        }
        let slot = match arg.as_str() {
            "--store" => &mut store,
            "--limit" => &mut limit,
            "--fields" => &mut fields,
            _ => return Err(format!("unknown search option {arg}")),
        };
        crate::set_once(
            slot,
            rest.next()
                .ok_or_else(|| format!("missing value for {arg}"))?
                .clone(),
            arg,
        )?;
    }
    let (selector, pattern) = match positional.as_slice() {
        [pattern] => ("label", *pattern), [selector, pattern] => (*selector, *pattern),
        _ => return Err("expected search [input|gate|final|label] PATTERN [--limit N] [--fields FIELD,...] [--store DIR]".into()),
    };
    let limit = limit
        .as_deref()
        .unwrap_or("10")
        .parse::<usize>()
        .map_err(|_| "--limit must be an integer from 1 through 1000")?;
    validate(selector, pattern, limit)?;
    let fields: Vec<_> = fields
        .as_deref()
        .map(|s| s.split(',').collect())
        .unwrap_or_else(|| FIELDS.to_vec());
    if fields.is_empty()
        || fields.iter().any(|field| !FIELDS.contains(field))
        || fields.iter().collect::<BTreeSet<_>>().len() != fields.len()
    {
        return Err(format!(
            "--fields must select distinct fields from {}",
            FIELDS.join(",")
        ));
    }
    if store.as_deref() == Some("") {
        return Err("--store must not be empty".into());
    }
    let root = store
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(storage::default_root)?;
    let session = profile::caller_session()?;
    let mut result = search(&root, &session, selector, pattern, limit)?;
    for hit in result["matches"].as_array_mut().unwrap() {
        hit.as_object_mut()
            .unwrap()
            .retain(|key, _| fields.contains(&key.as_str()));
    }
    println!("{result}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Store(PathBuf);
    impl Store {
        fn new() -> Self {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).unwrap();
            let path =
                std::env::temp_dir().join(format!("pira-search-{:x}", u128::from_le_bytes(random)));
            crate::private_dir(&path).unwrap();
            Self(path)
        }
        fn run(&self, id: &str, time: Option<u64>, owner: &str) -> PathBuf {
            let run = self.0.join(id);
            crate::private_dir(&run).unwrap();
            lifecycle::save_json(&run.join("manifest.json"), &json!({"run_id":id,
                "creator_main_session_id":owner,"parent_run_id":null,"revision":1,"status":"completed",
                "task":"input-only Shared [x].* İ λ", "completion_gate":"gate-only Shared",
                "revision_started_at_ms":time,"artifact":"artifacts/handoff","revisions":[]})).unwrap();
            crate::private_dir(&run.join("artifacts")).unwrap();
            fs::write(run.join("artifacts/handoff"), "final-only Shared").unwrap();
            run
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn selectors_are_literal_scoped_and_include_unbookmarked_runs() {
        let store = Store::new();
        let run = store.run("visible", Some(0), "main");
        store.run("hidden", Some(100), "other-main");
        fs::write(store.0.join("bookmarks.json"), r#"[2,[["main","Shared z","visible"],["main","Shared a","visible"],["other-main","Shared hidden","hidden"]],[]]"#).unwrap();
        for (selector, own, others) in [
            ("input", "input-only", ["gate-only", "final-only"]),
            ("gate", "gate-only", ["input-only", "final-only"]),
            ("final", "final-only", ["input-only", "gate-only"]),
        ] {
            let result = search(&store.0, "main", selector, own, 10).unwrap();
            assert_eq!(result["matches"][0]["run_id"], "visible");
            assert_eq!(result["matches"][0]["label"], "Shared a");
            for pattern in others {
                assert_eq!(
                    search(&store.0, "main", selector, pattern, 10).unwrap()["matches"],
                    json!([])
                );
            }
            assert_eq!(
                search(&store.0, "stranger", selector, "Shared", 10).unwrap()["matches"],
                json!([])
            );
        }
        assert_eq!(
            search(&store.0, "main", "label", "shared", 10).unwrap()["matches"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(hit(&run, "input", "[X].* İ Λ", None).unwrap().is_some());
        assert!(hit(&run, "input", "input.*", None).unwrap().is_none());
        fs::remove_file(store.0.join("bookmarks.json")).unwrap();
        assert!(
            search(&store.0, "main", "input", "shared", 10).unwrap()["matches"][0]["label"]
                .is_null()
        );
        assert_eq!(
            search(&store.0, "main", "label", "shared", 10).unwrap()["matches"],
            json!([])
        );
    }

    #[test]
    fn effective_assignment_and_latest_readable_handoff_keep_exact_provenance() {
        let store = Store::new();
        let run = store.run("run", Some(1000), "main");
        crate::private_dir(&run.join("revisions")).unwrap();
        crate::private_dir(&run.join("revisions/000001")).unwrap();
        let original = lifecycle::read_json(&run.join("manifest.json")).unwrap();
        lifecycle::save_json(&run.join("revisions/000001/manifest.json"), &original).unwrap();
        let mut current = original.clone();
        current["revision"] = json!(2);
        current["revision_started_at_ms"] = json!(2000);
        current["status"] = json!("running");
        current["task"] = json!("steered assignment");
        current["completion_gate"] = json!("steered gate");
        current["artifact"] = Value::Null;
        current["revisions"] = json!([{"revision":1}]);
        lifecycle::save_json(&run.join("manifest.json"), &current).unwrap();
        let input = hit(&run, "input", "steered", None).unwrap().unwrap();
        assert_eq!(input["revision"], 2);
        assert_eq!(input["status"], "running");
        assert_eq!(input["revision_at"], "1970-01-01T00:00:02.000Z");
        assert!(hit(&run, "input", "input-only", None).unwrap().is_none());
        assert!(hit(&run, "gate", "steered gate", None).unwrap().is_some());
        let final_hit = hit(&run, "final", "final-only", None).unwrap().unwrap();
        assert_eq!(final_hit["revision"], 1);
        assert_eq!(final_hit["status"], "completed");
        assert_eq!(final_hit["revision_at"], "1970-01-01T00:00:01.000Z");
        crate::private_dir(&run.join("revisions/000002")).unwrap();
        crate::private_dir(&run.join("revisions/000002/artifacts")).unwrap();
        current["status"] = json!("needs_decision");
        current["artifact"] = json!("revisions/000002/artifacts/handoff.json");
        fs::write(
            run.join("revisions/000002/artifacts/handoff.json"),
            r#"{"question":"new handoff"}"#,
        )
        .unwrap();
        lifecycle::save_json(&run.join("manifest.json"), &current).unwrap();
        assert!(hit(&run, "final", "final-only", None).unwrap().is_none()); // Not historical search.
        assert_eq!(
            hit(&run, "final", "question", None).unwrap().unwrap()["revision"],
            2
        );
        current["status"] = json!("incomplete");
        lifecycle::save_json(&run.join("manifest.json"), &current).unwrap();
        assert_eq!(
            hit(&run, "final", "new handoff", None).unwrap().unwrap()["status"],
            "incomplete"
        );
        fs::write(
            run.join("revisions/000002/artifacts/review-checkpoint.md"),
            "internal checkpoint",
        )
        .unwrap();
        current["artifact"] = json!("revisions/000002/artifacts/review-checkpoint.md");
        lifecycle::save_json(&run.join("manifest.json"), &current).unwrap();
        assert!(
            hit(&run, "final", "internal checkpoint", None)
                .unwrap()
                .is_none()
        );
        current["artifact"] = json!("revisions/000002/repair/candidate.txt");
        lifecycle::save_json(&run.join("manifest.json"), &current).unwrap();
        assert!(hit(&run, "final", "question", None).unwrap().is_none());
    }

    #[test]
    fn newest_distinct_runs_limit_ties_and_legacy_null_time() {
        let store = Store::new();
        for n in 0..14 {
            store.run(&format!("run-{n:02}"), Some(n * 1000), "main");
        }
        store.run("tie-a", Some(13000), "main");
        store.run("legacy", None, "main");
        let first = search(&store.0, "main", "input", "Shared", 10).unwrap();
        assert_eq!(first["matches"].as_array().unwrap().len(), 10);
        assert_eq!(first["has_more"], true);
        assert_eq!(first["matches"][0]["run_id"], "run-13");
        assert_eq!(first["matches"][1]["run_id"], "tie-a");
        let all = search(&store.0, "main", "input", "Shared", 1000).unwrap();
        assert_eq!(all["has_more"], false);
        assert_eq!(
            all["matches"].as_array().unwrap().last().unwrap()["run_id"],
            "legacy"
        );
        assert!(all["matches"].as_array().unwrap().last().unwrap()["revision_at"].is_null());
    }

    #[test]
    fn excerpts_use_original_unicode_and_nearby_text_with_strict_bound() {
        let text = format!(
            "{}İ {}NEEDLE{}",
            "a".repeat(400),
            "λ".repeat(80),
            "z".repeat(400)
        );
        let snippet = excerpt(&text, "needle").unwrap();
        assert!(snippet.contains("NEEDLE"));
        assert!(snippet.starts_with('λ'));
        assert_eq!(snippet.chars().count(), 240);
        assert!(
            excerpt("İx before\nNEEDLE\t after", "needle")
                .unwrap()
                .contains("before NEEDLE  after")
        );
        assert_eq!(excerpt("İX", "İx").unwrap(), "İX");
        assert!(
            excerpt(&format!("{}İ", "a".repeat(400)), "\u{307}")
                .unwrap()
                .ends_with('İ')
        );
        for (ms, expected) in [
            (0_u64, "1970-01-01T00:00:00.000Z"),
            (951827696789, "2000-02-29T12:34:56.789Z"),
            (253402300799999, "9999-12-31T23:59:59.999Z"),
        ] {
            assert_eq!(
                revision_time(&json!({"revision_started_at_ms":ms})).as_deref(),
                Some(expected)
            );
        }
        assert_eq!(
            revision_time(&json!({"revision_started_at_ms":u64::MAX})),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_paths_and_missing_ownership_do_not_expose_content() {
        use std::os::unix::fs::symlink;
        let store = Store::new();
        let run = store.run("owned", None, "main");
        symlink(&run, store.0.join("linked")).unwrap();
        let result = search(&store.0, "main", "input", "Shared", 10).unwrap();
        assert_eq!(result["matches"].as_array().unwrap().len(), 1);
        let outside = store.0.join("outside");
        fs::write(&outside, "private needle").unwrap();
        fs::remove_file(run.join("artifacts/handoff")).unwrap();
        symlink(&outside, run.join("artifacts/handoff")).unwrap();
        assert!(hit(&run, "final", "private", None).unwrap().is_none());
        let mut manifest = lifecycle::read_json(&run.join("manifest.json")).unwrap();
        manifest
            .as_object_mut()
            .unwrap()
            .remove("creator_main_session_id");
        lifecycle::save_json(&run.join("manifest.json"), &manifest).unwrap();
        store.run("collision", None, "main");
        crate::private_dir(&store.0.join(".remote-routes")).unwrap();
        crate::private_dir(&store.0.join(".remote-routes/collision")).unwrap();
        fs::write(store.0.join(".remote-routes/collision/route.json"), "{}").unwrap();
        assert_eq!(
            search(&store.0, "main", "input", "Shared", 10).unwrap()["matches"],
            json!([])
        );
    }
}
