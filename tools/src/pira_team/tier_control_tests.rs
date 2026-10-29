//! Model-free integration of the actual owner loop, schema validator and control socket.
use super::*;

#[test]
#[ignore = "requires Python 3 and PIRA_TIER_SCHEMA pointing to generated experimental schemas; no models/auth"]
fn tier_owner_loop_preserves_job_after_rejected_unknown_and_unavailable_updates() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!(
        "tier-owner-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("codex-home")).unwrap();
    fs::create_dir(root.join("scratch")).unwrap();
    fs::write(
        root.join("codex-home/config.toml"),
        "service_tier = \"fast\"\n",
    )
    .unwrap();
    fs::write(root.join("policy.md"), "fixture policy").unwrap();
    fs::write(root.join("phase.md"), "fixture phase").unwrap();
    let fixture = root.join("codex-fixture");
    fs::write(&fixture, include_str!("tier_fixture.py")).unwrap();
    fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700)).unwrap();
    let mut options = crate::parse_using(
        &[
            "run",
            "--task",
            "fixture",
            "--completion-gate",
            "fixture",
            "--cwd",
            root.to_str().unwrap(),
            "--store",
            root.to_str().unwrap(),
        ]
        .map(str::to_owned),
        || {
            Ok(
                json!({"approval_policy":"never","sandbox_policy":{"type":"danger-full-access"},
            "permission_profile":{"type":"disabled"}}),
            )
        },
    )
    .unwrap();
    options.authentication = crate::authentication::Authentication::SelectedCaller(fixture);
    let (done, finished) = mpsc::channel();
    let owner_root = root.clone();
    let owner = std::thread::spawn(move || {
        let mut manifest = json!({"revision":1,"subtree_epoch":0,"parent_run_id":null,
            "run_id":"fixture","repairs":0,"service_tier":"standard"});
        lifecycle::save_json(&owner_root.join("tree.json"), &json!({"version":1,"epoch":0,"blocked":false,
            "members":{owner_root.file_name().unwrap().to_str().unwrap():{"active":true,"revision":1,"blocked":false}}})).unwrap();
        let outcome = turn(
            &options,
            &owner_root,
            &owner_root,
            "fixture",
            &mut manifest,
            &owner_root.join("handoff"),
            (false, false, false, false),
        );
        done.send((outcome, manifest)).unwrap();
    });
    // Bounded readiness observation of task-local fixture metadata, never a real worker.
    let deadline = Instant::now() + Duration::from_secs(10);
    let endpoint = loop {
        if let Ok(endpoint) = lifecycle::read_json(&root.join("control.json"))
            && endpoint["turn_id"] == "turn"
        {
            break endpoint;
        }
        if let Ok((outcome, _)) = finished.try_recv() {
            panic!("fixture owner exited before readiness: {:?}", outcome.err());
        }
        assert!(Instant::now() < deadline, "fixture endpoint not ready");
        std::thread::sleep(Duration::from_millis(5));
    };
    let exchange = |tier: &str| {
        let mut stream = TcpStream::connect(endpoint["address"].as_str().unwrap()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        writeln!(stream,"{}",json!({"token":endpoint["token"],"operation":"tier","turn_id":"turn","service_tier":tier})).unwrap();
        read_frame(&mut stream).unwrap()
    };
    let pid = fs::read_to_string(root.join("job-running")).unwrap();
    for (tier, status, publication) in [
        ("fast", "accepted", "applied"),
        ("standard", "rejected", "rejected"),
        ("standard", "rejected", "targetUnavailable"),
        ("standard", "unknown", "unknown"),
        ("standard", "accepted", "applied"),
    ] {
        let response = exchange(tier);
        assert_eq!(response["status"], status, "{response}");
        assert_eq!(response["publication"], publication, "{response}");
        assert!(response["actual_service_tier"].is_null());
        assert!(
            matches!(finished.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "tier control stopped workload"
        );
        assert_eq!(fs::read_to_string(root.join("job-running")).unwrap(), pid);
        assert!(!root.join("job-finished").exists());
    }
    let late = std::thread::scope(|scope| {
        let pending = scope.spawn(|| exchange("fast"));
        let deadline = Instant::now() + Duration::from_secs(3);
        while !root.join("pending-tier").exists() {
            assert!(
                Instant::now() < deadline,
                "fixture tier publication not pending"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let busy = exchange("fast");
        assert_eq!(busy["status"], "rejected");
        assert!(busy["error"].as_str().unwrap().contains("pending"));
        assert!(!root.join("job-finished").exists());
        fs::OpenOptions::new()
            .write(true)
            .open(root.join("finish.fifo"))
            .unwrap()
            .write_all(b"finish")
            .unwrap();
        pending.join().unwrap()
    });
    assert!(
        late["error"]
            .as_str()
            .unwrap()
            .contains("delivery outcome unknown")
    );
    let (outcome, manifest) = finished.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(outcome.unwrap().status, "completed");
    owner.join().unwrap();
    assert!(root.join("job-finished").exists());
    assert_eq!(manifest["service_tier"], "standard");
    let retained = lifecycle::read_json(&root.join("manifest.json")).unwrap();
    assert_eq!(
        crate::service_tier::Tier::retained(&retained).unwrap(),
        crate::service_tier::Tier::Standard
    );
    let requests: Vec<Value> = fs::read_to_string(root.join("requests.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        requests
            .iter()
            .filter(|v| v["method"] == "turn/start")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .find(|v| v["method"] == "turn/start")
            .unwrap()["params"]["serviceTierForTurn"],
        "default"
    );
    assert!(
        requests
            .iter()
            .all(|v| !matches!(v["method"].as_str(), Some("turn/interrupt" | "turn/steer")))
    );
    let updates: Vec<_> = requests
        .iter()
        .filter(|v| v["method"] == "turn/settings/update")
        .collect();
    assert_eq!(updates.len(), 6);
    for update in updates {
        assert_eq!(update["params"].as_object().unwrap().len(), 3);
        assert_eq!(update["params"]["threadId"], "thread");
        assert_eq!(update["params"]["turnId"], "turn");
    }
    fs::remove_dir_all(root).unwrap();
}

fn closed_stdin_fixture_dir() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    loop {
        let root = std::env::temp_dir().join(format!(
            "tier-epipe-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create closed-stdin fixture {}: {error}", root.display()),
        }
    }
}

#[test]
fn closed_stdin_fixture_directories_are_unique_in_parallel() {
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                closed_stdin_fixture_dir()
            })
        })
        .collect();
    let roots: std::collections::BTreeSet<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(roots.len(), 4);
    for root in roots {
        fs::remove_dir(root).unwrap();
    }
}

// The same peer is used for no control, tier delivery failure, ordinary delivery
// failure and a subsequent real process failure. No model or authentication is used.
fn closed_stdin_probe(
    control: Option<&str>,
    finish: &str,
) -> (Value, Turn, bool, Value, Vec<Value>) {
    use std::os::unix::fs::PermissionsExt;
    let root = closed_stdin_fixture_dir();
    for name in ["codex-home", "scratch"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("policy.md"), "fixture policy").unwrap();
    fs::write(root.join("phase.md"), "fixture phase").unwrap();
    fs::write(root.join("close-stdin"), "fixture input half-close").unwrap();
    let fixture = root.join("codex-fixture");
    fs::write(&fixture, include_str!("tier_fixture.py")).unwrap();
    fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700)).unwrap();
    let mut options = crate::parse_using(
        &[
            "run",
            "--task",
            "fixture",
            "--completion-gate",
            "fixture",
            "--cwd",
            root.to_str().unwrap(),
            "--store",
            root.to_str().unwrap(),
        ]
        .map(str::to_owned),
        || {
            Ok(
                json!({"approval_policy":"never","sandbox_policy":{"type":"danger-full-access"},
            "permission_profile":{"type":"disabled"}}),
            )
        },
    )
    .unwrap();
    options.authentication = crate::authentication::Authentication::SelectedCaller(fixture);
    let (done, finished) = mpsc::channel();
    let owner_root = root.clone();
    let owner = std::thread::spawn(move || {
        let mut manifest = json!({"revision":1,"subtree_epoch":0,"parent_run_id":null,
            "run_id":"fixture","repairs":0,"service_tier":"standard"});
        lifecycle::save_json(&owner_root.join("tree.json"), &json!({"version":1,"epoch":0,"blocked":false,
            "members":{owner_root.file_name().unwrap().to_str().unwrap():{"active":true,"revision":1,"blocked":false}}})).unwrap();
        let outcome = turn(
            &options,
            &owner_root,
            &owner_root,
            "fixture",
            &mut manifest,
            &owner_root.join("handoff"),
            (false, false, false, false),
        );
        done.send((outcome, manifest)).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let endpoint = loop {
        if let Ok(endpoint) = lifecycle::read_json(&root.join("control.json"))
            && endpoint["turn_id"] == "turn"
            && root.join("input-closed").exists()
        {
            break endpoint;
        }
        if let Ok((outcome, _)) = finished.try_recv() {
            panic!("fixture exited before readiness: {:?}", outcome.err());
        }
        assert!(Instant::now() < deadline, "fixture did not close stdin");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(
        finished.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    let exchange = |operation: &str| {
        let mut stream = TcpStream::connect(endpoint["address"].as_str().unwrap()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        writeln!(
            stream,
            "{}",
            json!({"token":endpoint["token"],"operation":operation,"turn_id":"turn",
            "service_tier":"fast","task":"fixture steer","completion_gate":"fixture gate"})
        )
        .unwrap();
        read_frame(&mut stream).unwrap()
    };
    let receipt = control.map(exchange).unwrap_or(Value::Null);
    let ended = receipt["error"]
        .as_str()
        .is_some_and(|error| error.contains("run ended"));
    if control != Some("steer") && !ended {
        if control == Some("tier") {
            assert_eq!(receipt["status"], "unknown", "{receipt}");
            assert!(
                receipt["error"]
                    .as_str()
                    .unwrap()
                    .contains("delivery outcome unknown")
            );
            let next = exchange("tier");
            assert_eq!(next["status"], "rejected", "{next}");
            assert!(next["error"].as_str().unwrap().contains("stdin closed"));
            assert!(matches!(
                finished.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
            let pid: u32 = fs::read_to_string(root.join("job-running"))
                .unwrap()
                .parse()
                .unwrap();
            assert!(
                crate::native::identity(pid).is_ok(),
                "tier failure killed surviving backend"
            );
        }
        fs::OpenOptions::new()
            .write(true)
            .open(root.join("finish.fifo"))
            .unwrap()
            .write_all(finish.as_bytes())
            .unwrap();
    }
    let (outcome, manifest) = finished.recv_timeout(Duration::from_secs(5)).unwrap();
    owner.join().unwrap();
    let outcome = outcome.unwrap();
    let completed = root.join("job-finished").exists();
    let requests: Vec<Value> = fs::read_to_string(root.join("requests.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    println!(
        "closed-stdin control={control:?} finish={finish}: receipt={receipt}; workload={}; error={:?}; job_completed={completed}",
        outcome.status, outcome.error
    );
    fs::remove_dir_all(root).unwrap();
    (receipt, outcome, completed, manifest, requests)
}

#[test]
#[ignore = "requires Python 3 and PIRA_TIER_SCHEMA; model-free actual owner loop"]
fn closed_stdin_no_tier_workload_completes() {
    let (_, outcome, completed, _, _) = closed_stdin_probe(None, "finish");
    assert_eq!(outcome.status, "completed");
    assert!(outcome.error.is_none());
    assert!(completed);
}

#[test]
#[ignore = "requires Python 3 and PIRA_TIER_SCHEMA; model-free actual owner loop"]
fn closed_stdin_tier_failure_preserves_active_workload() {
    let (receipt, outcome, completed, manifest, requests) =
        closed_stdin_probe(Some("tier"), "finish");
    assert_eq!(receipt["status"], "unknown");
    assert_eq!(outcome.status, "completed");
    assert!(outcome.error.is_none());
    assert!(completed);
    assert_eq!(manifest["service_tier"], "standard");
    assert!(manifest["tier_publication"].is_null());
    assert_eq!(
        requests
            .iter()
            .filter(|v| v["method"] == "turn/start")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|v| v["method"] == "turn/settings/update")
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .all(|v| !matches!(v["method"].as_str(), Some("turn/interrupt" | "turn/steer")))
    );
}

#[test]
#[ignore = "requires Python 3 and PIRA_TIER_SCHEMA; model-free actual owner loop"]
fn closed_stdin_ordinary_write_failure_remains_fatal() {
    let (_, outcome, completed, _, _) = closed_stdin_probe(Some("steer"), "finish");
    assert_eq!(outcome.status, "failed");
    assert!(outcome.error.unwrap().contains("write app-server request"));
    assert!(!completed);
}

#[test]
#[ignore = "requires Python 3 and PIRA_TIER_SCHEMA; model-free actual owner loop"]
fn closed_stdin_tier_failure_does_not_hide_later_backend_exit() {
    let (_, outcome, completed, _, _) = closed_stdin_probe(Some("tier"), "exit");
    assert_eq!(outcome.status, "failed");
    assert!(outcome.error.unwrap().contains("closed stdout"));
    assert!(!completed);
}
