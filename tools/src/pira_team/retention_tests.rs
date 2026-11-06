use super::*;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

const NOW: u64 = 1_700_000_000_000;
struct Store(PathBuf);
impl Store {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "team-retention-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::private_dir(&path).unwrap();
        Self(path)
    }
    fn run(&self, id: &str, last: Option<u64>) -> PathBuf {
        let path = self.0.join(id);
        crate::private_dir(&path).unwrap();
        crate::create(&path.join("run.lock")).unwrap();
        let mut manifest =
            json!({"run_id":id,"revision":1,"status":"completed","parent_run_id":null});
        if let Some(last) = last {
            manifest["last_execution_activity_ms"] = json!(last);
        }
        lifecycle::save_json(&path.join("manifest.json"), &manifest).unwrap();
        path
    }
    fn tree(&self) {
        for (id, parent) in [("root", Value::Null), ("child", json!("root"))] {
            let p = self.run(id, Some(NOW - AGE_MS - 1));
            let mut m = read(&p.join("manifest.json")).unwrap();
            m["parent_run_id"] = parent;
            m["allow_subteam"] = json!(id == "root");
            lifecycle::save_json(&p.join("manifest.json"), &m).unwrap();
        }
        crate::create(&self.0.join("root/tree.lock")).unwrap();
        lifecycle::save_json(&self.0.join("root/tree.json"),&json!({"version":1,"epoch":0,"blocked":false,
            "members":{"root":{"active":false,"revision":1},"child":{"active":false,"revision":1}}})).unwrap();
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn change(run: &Path, edit: impl FnOnce(&mut Value)) {
    let path = run.join("manifest.json");
    let mut m = read(&path).unwrap();
    edit(&mut m);
    lifecycle::save_json(&path, &m).unwrap();
}
fn log(dir: &Path, time: u64) {
    let file = crate::create(&dir.join("events.jsonl")).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(time)))
        .unwrap();
}
fn unit(id: &str, last: u64, bytes: u64) -> Unit {
    Unit {
        id: id.into(),
        members: vec![id.into()],
        last: Some(last),
        size: Size {
            bytes,
            unknown: false,
        },
        skip: None,
        reasons: Vec::new(),
        retiring: false,
        proofs: None,
    }
}
fn planned(store: &Store) -> Plan {
    plan(&store.0, NOW).unwrap()
}

fn intent(store: &Store, unit: &Unit) {
    lifecycle::save_json(
        &marker(&store.0, &unit.id),
        &json!({"version":2,"root_id":unit.id,"members":unit.members,
        "last_activity_ms":unit.last,"reasons":["age"],"proofs":proofs(&store.0,unit).unwrap()}),
    )
    .unwrap();
}

fn recover(root: &Path) -> Vec<Value> {
    let result = execute(root, false).unwrap();
    let mut rows = result["outcomes"].as_array().unwrap().clone();
    for row in &mut rows {
        row["recovered"] = row["deleted"].clone();
    }
    rows.extend(
        result["recovery_issues"]
            .as_array()
            .unwrap()
            .iter()
            .cloned(),
    );
    rows
}

#[test]
fn exact_age_and_decimal_budget_boundaries_and_execution_order() {
    let mut age = vec![
        unit("equal", NOW - AGE_MS, 1),
        unit("older", NOW - AGE_MS - 1, 1),
        unit("younger", NOW - AGE_MS + 1, 1),
    ];
    assert_eq!(select(&mut age, 3, NOW, BUDGET), 1);
    assert_eq!(
        age.iter().find(|u| u.id == "older").unwrap().reasons,
        vec!["age"]
    );
    for total in [BUDGET - 1, BUDGET, BUDGET + 1] {
        let mut units = vec![
            unit("created-first", NOW - 10, 2),
            unit("created-last", NOW - 20, 2),
        ];
        assert_eq!(
            select(&mut units, total, NOW, BUDGET),
            if total < BUDGET { 0 } else { 2 }
        );
        assert_eq!(units[0].id, "created-last");
        assert_eq!(units[0].reasons.is_empty(), total < BUDGET);
    }
    let mut protected = unit("live", NOW - 100, BUDGET);
    protected.skip = Some("live_owner".into());
    let mut units = vec![protected, unit("idle", NOW - 10, 2)];
    assert_eq!(select(&mut units, BUDGET + 2, NOW, BUDGET), 2);
    assert_eq!(
        units.iter().find(|u| u.id == "live").unwrap().reasons.len(),
        0
    );
}

#[test]
fn legacy_latest_revision_stage_repair_logs_and_persisted_times() {
    let store = Store::new();
    let run = store.run("legacy", None);
    log(&run, NOW - AGE_MS - 20);
    assert_eq!(planned(&store).units[0].last, Some(NOW - AGE_MS - 20));
    // Directory, artifact, and read access mtimes do not participate.
    fs::write(run.join("report.md"), "unrelated").unwrap();
    fs::read(run.join("manifest.json")).unwrap();
    assert_eq!(planned(&store).units[0].last, Some(NOW - AGE_MS - 20));
    crate::private_dir(&run.join("revisions")).unwrap();
    crate::private_dir(&run.join("revisions/000002")).unwrap();
    change(&run, |m| m["revision"] = json!(2));
    assert_eq!(
        planned(&store).units[0].skip.as_deref(),
        Some("unknown_activity")
    );
    let repair = run.join("revisions/000002/implementation");
    crate::private_dir(&repair).unwrap();
    crate::private_dir(&repair.join("repair")).unwrap();
    log(&repair.join("repair"), NOW - 10);
    assert_eq!(planned(&store).units[0].last, Some(NOW - 10));
    change(&run, |m| m["resumed_at_ms"] = json!(NOW - 5));
    assert_eq!(planned(&store).units[0].last, Some(NOW - 5));
    change(&run, |m| m["resumed_at_ms"] = json!(NOW + 1));
    assert_eq!(
        planned(&store).units[0].skip.as_deref(),
        Some("invalid_activity")
    );
}

#[test]
fn activity_updates_at_execution_only_and_uses_later_run_log() {
    let store = Store::new();
    let run = store.run("resumed", Some(NOW - AGE_MS - 1));
    assert_eq!(planned(&store).reclaim, planned(&store).total.bytes);
    change(&run, |m| m["last_execution_activity_ms"] = json!(NOW - 5));
    log(&run, NOW - 1);
    assert_eq!(planned(&store).units[0].last, Some(NOW - 1));
    let mut m = read(&run.join("manifest.json")).unwrap();
    activity(&mut m).unwrap();
    assert!(m["last_execution_activity_ms"].as_u64().unwrap() >= 1_700_000_000_000);
}

#[test]
fn coherent_unit_max_activity_owner_and_relationship_protection() {
    let store = Store::new();
    store.tree();
    let plan = planned(&store);
    assert_eq!(plan.units.len(), 1);
    assert_eq!(plan.units[0].members.len(), 2);
    change(&store.0.join("child"), |m| {
        m["last_execution_activity_ms"] = json!(NOW - 1)
    });
    assert_eq!(planned(&store).reclaim, 0);
    // Eligible units still require native owner exclusion before selection.
    change(&store.0.join("child"), |m| {
        m["last_execution_activity_ms"] = json!(NOW - AGE_MS - 1)
    });
    let owner = lock(&store.0.join("child/run.lock"), false).unwrap();
    assert_eq!(
        planned(&store).units[0].skip.as_deref(),
        Some("live_owner_or_busy_lock")
    );
    drop(owner);
    change(&store.0.join("child"), |m| {
        m["parent_run_id"] = json!("../escape")
    });
    assert!(planned(&store).units.iter().all(|u| u.skip.is_some()));
}

#[test]
fn terminal_status_needs_owner_and_native_evidence_running_without_it_is_unknown() {
    let store = Store::new();
    let run = store.run("failed", Some(NOW - AGE_MS - 1));
    change(&run, |m| m["status"] = json!("failed"));
    assert!(planned(&store).reclaim > 0);
    change(&run, |m| m["status"] = json!("interrupted"));
    assert!(planned(&store).reclaim > 0);
    change(&run, |m| m["status"] = json!("running"));
    assert_eq!(
        planned(&store).units[0].skip.as_deref(),
        Some("unknown_native_state")
    );
    let me = crate::native::current().unwrap();
    lifecycle::save_json(
        &run.join("runtime.json"),
        &json!({"revision":1,"launcher_pid":me.pid,"launcher_birth":me.birth}),
    )
    .unwrap();
    assert_eq!(
        planned(&store).units[0].skip.as_deref(),
        Some("live_launcher")
    );
    change(&run, |m| m["status"] = json!("completed"));
    assert_eq!(
        planned(&store).units[0].skip.as_deref(),
        Some("live_launcher")
    );
}

fn fingerprint(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let meta = fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            result.extend(fingerprint(&path));
            result.insert(path, (Vec::new(), meta.modified().unwrap()));
        } else {
            result.insert(
                path.clone(),
                (
                    if meta.len() == 0 {
                        Vec::new()
                    } else {
                        fs::read(&path).unwrap()
                    },
                    meta.modified().unwrap(),
                ),
            );
        }
    }
    result.insert(
        root.to_owned(),
        (Vec::new(), fs::metadata(root).unwrap().modified().unwrap()),
    );
    result
}

#[test]
fn dry_run_is_immutable_and_bookmarks_never_exempt_or_lose_unrelated_entries() {
    let store = Store::new();
    store.run("old", Some(now_ms().unwrap() - AGE_MS - 100));
    store.run("keep", Some(now_ms().unwrap()));
    fs::write(store.0.join("bookmarks.json"),b"[2,[[\"session\",\"chosen\",\"old\"],[\"session\",\"keep\",\"keep\"]],[[\"legacy\",\"old\"]]]\n").unwrap();
    let before = fingerprint(&store.0);
    let preview = execute(&store.0, true).unwrap();
    assert_eq!(preview["summary"]["selected_members"], 1);
    assert_eq!(fingerprint(&store.0), before);
    let actual = execute(&store.0, false).unwrap();
    assert_eq!(actual["outcomes"][0]["deleted"], true, "{actual}");
    assert!(!store.0.join("old").exists());
    assert!(store.0.join("keep").exists());
    let bookmarks: Value =
        serde_json::from_slice(&fs::read(store.0.join("bookmarks.json")).unwrap()).unwrap();
    assert_eq!(bookmarks, json!([2, [["session", "keep", "keep"]], []]));
}

#[test]
fn changed_activity_and_busy_resume_between_plan_and_delete_are_not_removed() {
    let store = Store::new();
    let run = store.run("old", Some(NOW - AGE_MS - 1));
    let unit = planned(&store).units.remove(0);
    let owner = crate::lifecycle::owner(&run).unwrap();
    assert_eq!(
        retire(&store.0, &unit, NOW).unwrap_err(),
        "live_owner_or_busy_lock"
    );
    drop(owner);
    change(&run, |m| m["last_execution_activity_ms"] = json!(NOW));
    assert_eq!(
        retire(&store.0, &unit, NOW).unwrap_err(),
        "activity_changed_after_plan"
    );
    assert!(run.exists());
}

#[test]
fn interrupted_payload_deletion_recovers_as_one_unit_and_fences_resume_admission() {
    let store = Store::new();
    store.tree();
    let unit = planned(&store).units.remove(0);
    intent(&store, &unit);
    fs::remove_file(store.0.join("root/manifest.json")).unwrap();
    assert!(crate::lifecycle::owner(&store.0.join("root")).is_err());
    let recovered = recover(&store.0);
    assert_eq!(recovered[0]["recovered"], true, "{recovered:?}");
    assert!(!store.0.join("root").exists());
    assert!(!store.0.join("child").exists());
    assert!(!marker(&store.0, "root").exists());
}

#[test]
fn bookmark_error_keeps_journal_and_unrelated_bookmarks_then_recovers() {
    let store = Store::new();
    let run = store.run("old", Some(NOW - AGE_MS - 1));
    let unit = planned(&store).units.remove(0);
    fs::write(store.0.join("bookmarks.json"), "invalid").unwrap();
    assert!(retire(&store.0, &unit, NOW).is_err());
    assert!(run.exists());
    assert!(marker(&store.0, "old").exists());
    fs::write(store.0.join("bookmarks.json"), "[2,[],[]]\n").unwrap();
    let recovered = recover(&store.0);
    assert_eq!(recovered[0]["recovered"], true, "{recovered:?}");
    assert!(!run.exists());
    // Crash after payload removal but before journal removal is also recoverable.
    lifecycle::save_json(
        &marker(&store.0, "gone"),
        &json!({"version":2,"root_id":"gone","members":["gone"],"last_activity_ms":1,"reasons":["age"],"proofs":{"gone":{"revision":1,"status":"completed","runtime":null}}}),
    )
    .unwrap();
    let recovered = recover(&store.0);
    assert_eq!(recovered[0]["recovered"], true, "{recovered:?}");
}

#[test]
fn malformed_journal_and_manifest_ids_never_select_external_paths() {
    let store = Store::new();
    let other = Store::new();
    let run = store.run("old", Some(NOW - AGE_MS - 1));
    change(&run, |m| m["run_id"] = json!("../escape"));
    assert_eq!(
        planned(&store).units[0].skip.as_deref(),
        Some("unit_relationships")
    );
    lifecycle::save_json(
        &marker(&store.0, "old"),
        &json!({"version":1,"root_id":"old","members":["old","../escape"],"last_activity_ms":1,"reasons":["age"]}),
    )
    .unwrap();
    let before = fingerprint(&other.0);
    assert!(recover(&store.0)[0]["error"].is_string());
    assert_eq!(before, fingerprint(&other.0));
    assert!(run.exists());
}

#[cfg(unix)]
#[test]
fn symlinks_are_never_traversed_for_timing_size_or_deletion() {
    use std::os::unix::fs::symlink;
    let store = Store::new();
    let other = Store::new();
    let outside = other.run("outside", Some(NOW));
    symlink(&outside, store.0.join("linked")).unwrap();
    let run = store.run("old", Some(NOW - AGE_MS - 1));
    symlink(&other.0, run.join("scratch")).unwrap();
    let plan = planned(&store);
    assert!(
        plan.units
            .iter()
            .find(|u| u.id == "linked")
            .unwrap()
            .skip
            .is_some()
    );
    let unit = plan.units.iter().find(|u| u.id == "old").unwrap();
    let before = fingerprint(&other.0);
    retire(&store.0, unit, NOW).unwrap();
    assert_eq!(before, fingerprint(&other.0));
    assert!(execute(&store.0.join("linked"), true).is_err());
}

#[test]
fn cleaner_lock_is_nonblocking_and_skips_without_scanning() {
    let store = Store::new();
    store.run("old", Some(now_ms().unwrap() - AGE_MS - 10));
    let cleaner = lock(&store.0.join(".cleanup.lock"), true).unwrap();
    let before = fingerprint(&store.0);
    let start = Instant::now();
    let result = execute(&store.0, false).unwrap();
    assert_eq!(result["skipped"], "cleaner_already_active");
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(fingerprint(&store.0), before);
    drop(cleaner);
    assert_eq!(
        execute(&store.0, false).unwrap()["outcomes"][0]["deleted"],
        true
    );
}

// Real detached process helper: TCP gives deterministic cross-platform readiness
// and release. It is present only in this test executable, never production code.
#[test]
fn detached_helper() {
    let Ok(mode) = std::env::var("TEAM_RETENTION_HELPER") else {
        return;
    };
    if mode == "origin" {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "retention::tests::detached_helper",
            "--nocapture",
        ])
        .env("TEAM_RETENTION_HELPER", "cleaner")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(
            crate::create(
                &PathBuf::from(std::env::var_os("TEAM_RETENTION_STORE").unwrap())
                    .join(".helper-stderr"),
            )
            .unwrap(),
        );
        detached(&mut cmd).unwrap();
        return;
    }
    if mode == "proof" {
        let mut stream =
            std::net::TcpStream::connect(std::env::var("TEAM_RETENTION_READY").unwrap()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(b"ready").unwrap();
        let mut release = [0];
        stream.read_exact(&mut release).unwrap();
        return;
    }
    let root = PathBuf::from(std::env::var_os("TEAM_RETENTION_STORE").unwrap());
    let held = lock(&root.join(".cleanup.lock"), true).unwrap();
    let mut stream =
        std::net::TcpStream::connect(std::env::var("TEAM_RETENTION_READY").unwrap()).unwrap();
    stream.write_all(&std::process::id().to_be_bytes()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let mut release = [0];
    stream.read_exact(&mut release).unwrap();
    drop(held);
    let result = execute(&root, false).unwrap();
    assert_eq!(result["summary"]["selected_units"], 0);
    stream.write_all(b"done").unwrap();
}

#[test]
fn detached_process_survives_origin_exit_releases_lock_and_terminates() {
    let store = Store::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "retention::tests::detached_helper",
            "--nocapture",
        ])
        .env("TEAM_RETENTION_HELPER", "origin")
        .env("TEAM_RETENTION_STORE", &store.0)
        .env(
            "TEAM_RETENTION_READY",
            listener.local_addr().unwrap().to_string(),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // The same native scope used by Team permits deliberate breakaway. Cargo's
    // external job may deny direct escape; this fixture owns the scope it kills.
    let (mut origin, origin_scope) = crate::process::spawn(&mut command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut connection = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => {
                let status = origin.try_wait().unwrap();
                if status.is_none() {
                    let _ = origin.kill();
                }
                let output = origin.wait_with_output().unwrap();
                panic!(
                    "cleaner readiness: {e}; origin={:?}; stderr={}; helper={}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr),
                    fs::read_to_string(store.0.join(".helper-stderr")).unwrap_or_default()
                );
            }
        }
    };
    connection.set_nonblocking(false).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut bytes = [0; 4];
    connection.read_exact(&mut bytes).unwrap();
    let pid = u32::from_be_bytes(bytes);
    assert!(origin_scope.wait(&mut origin).unwrap().success());
    drop(origin_scope);
    assert!(
        crate::native::identity(pid).unwrap().alive,
        "cleaner must outlive its origin"
    );
    assert_eq!(
        execute(&store.0, false).unwrap()["skipped"],
        "cleaner_already_active"
    );
    connection.write_all(b"x").unwrap();
    let mut done = [0; 4];
    connection.read_exact(&mut done).unwrap();
    assert_eq!(&done, b"done");
    let mut eof = [0];
    assert_eq!(connection.read(&mut eof).unwrap(), 0);
    // Stream closure follows cleaner work; lock ownership was explicitly released.
    let _lock = lock(&store.0.join(".cleanup.lock"), false).unwrap();
}

#[test]
fn dry_run_reports_pending_recovery_and_partial_payload_without_mutation() {
    let store = Store::new();
    store.tree();
    let unit = planned(&store).units.remove(0);
    intent(&store, &unit);
    fs::remove_file(store.0.join("root/manifest.json")).unwrap();
    fs::remove_file(store.0.join("child/manifest.json")).unwrap();
    fs::remove_file(store.0.join("child/run.lock")).unwrap();
    let before = fingerprint(&store.0);
    let preview = execute(&store.0, true).unwrap();
    assert_eq!(before, fingerprint(&store.0));
    assert_eq!(preview["units"][0]["pending_recovery"], true);
    assert_eq!(preview["summary"]["selected_members"], 2);
    let actual = execute(&store.0, false).unwrap();
    assert_eq!(actual["units"], preview["units"]);
    assert_eq!(actual["outcomes"][0]["deleted"], true, "{actual}");
}

#[cfg(unix)]
#[test]
fn removal_failure_preserves_journal_and_recovers_without_unrelated_loss() {
    use std::os::unix::fs::PermissionsExt;
    let store = Store::new();
    let other = store.run("keep", Some(now_ms().unwrap()));
    let run = store.run("old", Some(NOW - AGE_MS - 1));
    let protected = run.join("scratch");
    crate::private_dir(&protected).unwrap();
    fs::write(protected.join("payload"), b"fixture").unwrap();
    fs::set_permissions(&protected, fs::Permissions::from_mode(0o500)).unwrap();
    let unit = planned(&store)
        .units
        .into_iter()
        .find(|u| u.id == "old")
        .unwrap();
    let before = fingerprint(&other);
    let error = retire(&store.0, &unit, NOW).unwrap_err();
    assert_eq!(error, "retirement_remove_failed");
    assert!(marker(&store.0, "old").exists());
    assert_eq!(before, fingerprint(&other));
    fs::set_permissions(
        store.0.join("old/scratch"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let recovered = recover(&store.0);
    assert_eq!(recovered[0]["recovered"], true, "{recovered:?}");
    assert!(!marker(&store.0, "old").exists());
    assert_eq!(before, fingerprint(&other));
}

#[cfg(unix)]
#[test]
fn symlink_manifest_owner_tree_and_log_metadata_fail_closed() {
    use std::os::unix::fs::symlink;
    for target in ["manifest.json", "run.lock", "tree.lock", "events.jsonl"] {
        let store = Store::new();
        let other = Store::new();
        store.tree();
        let run = store.0.join("root");
        let path = run.join(target);
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        let external = other.0.join("sentinel");
        fs::write(&external, b"external").unwrap();
        symlink(&external, &path).unwrap();
        let before = fingerprint(&other.0);
        let plan = planned(&store);
        assert_eq!(plan.reclaim, 0, "{target}");
        assert_eq!(before, fingerprint(&other.0));
    }
}

#[cfg(unix)]
#[test]
fn nonregular_scratch_size_is_unknown_and_never_blocks_inspection() {
    use std::os::unix::ffi::OsStrExt;
    let store = Store::new();
    let run = store.run("unknown", Some(NOW - AGE_MS - 1));
    let path = std::ffi::CString::new(run.join("pipe").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let start = Instant::now();
    let plan = planned(&store);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(plan.total.unknown);
    assert_eq!(plan.reclaim, 0);
    assert_eq!(plan.units[0].skip.as_deref(), Some("unknown_size"));
}

#[test]
fn malformed_bookmarks_fail_dry_run_and_cleanup_before_payload_removal() {
    let store = Store::new();
    store.run("old", Some(now_ms().unwrap() - AGE_MS - 100));
    fs::write(store.0.join("bookmarks.json"), b"invalid").unwrap();
    let before = fingerprint(&store.0);
    assert!(execute(&store.0, true).is_err());
    assert_eq!(before, fingerprint(&store.0));
    assert!(execute(&store.0, false).is_err());
    assert!(store.0.join("old").is_dir());
    assert!(!marker(&store.0, "old").exists());
    assert_eq!(
        fs::read(store.0.join("bookmarks.json")).unwrap(),
        b"invalid"
    );
}

#[test]
fn ineligible_young_units_are_screened_without_owner_lock_contention() {
    let store = Store::new();
    let run = store.run("young", Some(NOW));
    let before = planned(&store).units[0].value();
    let _owner = crate::lifecycle::owner(&run).unwrap();
    assert_eq!(
        planned(&store).units[0].value(),
        before,
        "read-only screening of an ineligible unit must not contend for execution ownership"
    );
}

#[test]
fn exited_native_scope_proof_survives_partial_metadata_deletion() {
    let store = Store::new();
    let run = store.run("orphan", Some(now_ms().unwrap() - AGE_MS - 100));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "retention::tests::detached_helper",
            "--nocapture",
        ])
        .env("TEAM_RETENTION_HELPER", "proof")
        .env(
            "TEAM_RETENTION_READY",
            listener.local_addr().unwrap().to_string(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut child, scope) = crate::process::spawn(&mut command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("proof peer: {e}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .unwrap();
    let mut ready = [0; 5];
    stream.read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"ready");
    let identity = crate::native::identity(child.id()).unwrap();
    #[cfg(windows)]
    let session = Some(crate::native::windows::session_id(child.id()).unwrap());
    #[cfg(not(windows))]
    let session: Option<u32> = None;
    lifecycle::save_json(&run.join("runtime.json"),&json!({"revision":1,"launcher_pid":identity.pid,"launcher_birth":identity.birth,
        "backend_pid":identity.pid,"backend_birth":identity.birth,"backend_job":scope.job_name(),"backend_job_session":session})).unwrap();
    change(&run, |m| m["status"] = json!("running"));
    assert!(execute(&store.0, true).unwrap()["units"][0]["skip"].is_string());
    stream.write_all(b"x").unwrap();
    assert!(scope.wait(&mut child).unwrap().success());
    drop(scope);
    let mut unit = plan(&store.0, now_ms().unwrap()).unwrap().units.remove(0);
    assert!(unit.skip.is_none(), "{}", unit.value());
    unit.reasons = vec!["age"];
    intent(&store, &unit);
    fs::remove_file(run.join("manifest.json")).unwrap();
    fs::remove_file(run.join("runtime.json")).unwrap();
    let result = execute(&store.0, false).unwrap();
    assert_eq!(result["outcomes"][0]["deleted"], true, "{result}");
    assert!(!run.exists());
}
