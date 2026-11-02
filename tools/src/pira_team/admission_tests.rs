fn resume_fixture() -> (TestDir, PathBuf, Value, Value) {
    let dir = TestDir::new();
    let run = dir.0.join("run");
    private_dir(&run).unwrap();
    private_dir(&run.join("revisions")).unwrap();
    private_dir(&run.join("revisions/000001")).unwrap();
    let manifest = json!({"run_id":"run","revision":1,"status":"completed",
        "creator_main_session_id":"fixture-root","parent_run_id":null,"subtree_epoch":7,
        "thread_id":"fixture-thread","usage":{},"revisions":[{"revision":1,"status":"completed"}]});
    let tree = json!({"version":1,"epoch":7,"blocked":false,
        "members":{"run":{"active":false,"revision":1,"blocked":false,
            "launcher_pid":123,"launcher_birth":456,"credential_claimed":true}}});
    save_json(&run.join("manifest.json"), &manifest).unwrap();
    save_json(&run.join("revisions/000001/manifest.json"), &manifest).unwrap();
    save_json(&run.join("tree.json"), &tree).unwrap();
    (dir, run, manifest, tree)
}

#[test]
fn failed_resume_publication_undoes_admission_and_allows_guarded_recovery() {
    let (_dir, run, original, tree) = resume_fixture();
    let original_bytes = fs::read(run.join("manifest.json")).unwrap();
    let owner = super::owner(&run).unwrap();
    let obstacle = run.join(format!("manifest.{}.tmp", std::process::id()));
    fs::write(&obstacle, b"not owned by this revision").unwrap();
    let mut next = original.clone();
    next["revision"] = json!(2);
    let mut options = crate::tests::options(&[
        "fixture",
        "--store",
        run.parent().unwrap().to_str().unwrap(),
    ])
    .unwrap();
    let error = admitted_revision(&mut options, &run, &mut next, &owner).unwrap_err();
    assert!(error.contains("manifest."));
    assert!(!error.contains("admission retained"), "{error}");
    assert_eq!(fs::read(run.join("manifest.json")).unwrap(), original_bytes);
    assert_eq!(
        fs::read(run.join("revisions/000001/manifest.json")).unwrap(),
        original_bytes
    );
    assert_eq!(read_json(&run.join("tree.json")).unwrap(), tree);
    assert!(!run.join("revisions/000002").exists());
    assert_eq!(fs::read(&obstacle).unwrap(), b"not owned by this revision");
    fs::remove_file(obstacle).unwrap();
    drop(owner);
    assert_eq!(
        crate::subteam::interrupt(&run, true).unwrap()["status"],
        "stopped"
    );
    // Exercise supported admission/publication after explicit interruption, without a backend.
    let owner = super::owner(&run).unwrap();
    next = original;
    next["revision"] = json!(2);
    let admission = crate::subteam::admit(&run, &mut next, true, &owner).unwrap();
    next["status"] = json!("running");
    begin_revision(&run.join("revisions/000002"), || {
        save_json(&run.join("manifest.json"), &next)
    })
    .unwrap();
    admission.undo_unpublished().unwrap();
    let current = read_json(&run.join("tree.json")).unwrap();
    assert_eq!(current["blocked"], false);
    assert_eq!(current["epoch"], 8);
    assert_eq!(current["members"]["run"]["active"], true);
    assert_eq!(current["members"]["run"]["revision"], 2);
    assert_eq!(read_json(&run.join("manifest.json")).unwrap(), next);
}

#[test]
fn failed_resume_allocation_preserves_payload_and_restores_only_admission() {
    let (_dir, run, mut next, tree) = resume_fixture();
    let owner = owner(&run).unwrap();
    let snapshot = run.join("revisions/000002");
    private_dir(&snapshot).unwrap();
    fs::write(snapshot.join("keep"), b"unrelated payload").unwrap();
    next["revision"] = json!(2);
    let mut options = crate::tests::options(&[
        "fixture",
        "--store",
        run.parent().unwrap().to_str().unwrap(),
    ])
    .unwrap();
    assert!(admitted_revision(&mut options, &run, &mut next, &owner).is_err());
    assert_eq!(read_json(&run.join("tree.json")).unwrap(), tree);
    assert_eq!(
        fs::read(snapshot.join("keep")).unwrap(),
        b"unrelated payload"
    );
    assert_eq!(
        read_json(&run.join("manifest.json")).unwrap()["revision"],
        1
    );
}

#[test]
fn admission_undo_waits_for_tree_lock_and_preserves_concurrent_gates() {
    let (_dir, run, mut next, original_tree) = resume_fixture();
    let owner = owner(&run).unwrap();
    next["revision"] = json!(2);
    let admission = crate::subteam::admit(&run, &mut next, true, &owner).unwrap();
    let other_run = run.clone();
    let (ready, acquired) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let concurrent = std::thread::spawn(move || {
        let _lock = crate::subteam::lock(&other_run).unwrap();
        ready.send(()).unwrap();
        wait.recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        let mut tree = read_json(&other_run.join("tree.json")).unwrap();
        tree["blocked"] = json!(true);
        tree["members"]["run"]["blocked"] = json!(true);
        tree["members"]["other"] = json!({"active":false,"revision":4,"blocked":true});
        save_json(&other_run.join("tree.json"), &tree).unwrap();
    });
    acquired
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    crate::subteam::on_lock_contention(move || release.send(()).unwrap());
    admission.undo_unpublished().unwrap();
    concurrent.join().unwrap();
    let current = read_json(&run.join("tree.json")).unwrap();
    assert_eq!(current["blocked"], true);
    assert_eq!(current["epoch"], 7);
    let mut previous = original_tree["members"]["run"].clone();
    previous["blocked"] = json!(true);
    assert_eq!(current["members"]["run"], previous);
    assert_eq!(
        current["members"]["other"],
        json!({"active":false,"revision":4,"blocked":true})
    );
}

#[test]
fn admission_undo_refuses_changed_epoch_identity_or_claim() {
    for (field, value) in [
        ("epoch", json!(8)),
        ("revision", json!(3)),
        ("launcher_pid", json!(1)),
        ("launcher_birth", json!(0)),
        ("credential_claimed", json!(true)),
        ("active", json!(false)),
        ("unknown", json!(true)),
    ] {
        let (_dir, run, mut next, _) = resume_fixture();
        let owner = owner(&run).unwrap();
        next["revision"] = json!(2);
        let admission = crate::subteam::admit(&run, &mut next, true, &owner).unwrap();
        let mut changed = read_json(&run.join("tree.json")).unwrap();
        if field == "epoch" {
            changed["epoch"] = value;
        } else {
            changed["members"]["run"][field] = value;
        }
        save_json(&run.join("tree.json"), &changed).unwrap();
        assert!(admission.undo_unpublished().is_err(), "{field}");
        assert_eq!(
            read_json(&run.join("tree.json")).unwrap(),
            changed,
            "{field}"
        );
    }
}

#[test]
fn admission_undo_never_reverts_published_or_changed_manifest() {
    for status in ["running", "completed", "incomplete"] {
        let (_dir, run, mut next, _) = resume_fixture();
        let owner = owner(&run).unwrap();
        next["revision"] = json!(2);
        let admission = crate::subteam::admit(&run, &mut next, true, &owner).unwrap();
        next["status"] = json!(status);
        save_json(&run.join("manifest.json"), &next).unwrap();
        let admitted = read_json(&run.join("tree.json")).unwrap();
        admission.undo_unpublished().unwrap();
        assert_eq!(read_json(&run.join("manifest.json")).unwrap(), next);
        assert_eq!(read_json(&run.join("tree.json")).unwrap(), admitted);
    }
}

#[test]
fn admission_undo_is_disarmed_once_publication_is_acknowledged() {
    let (_dir, run, mut next, _) = resume_fixture();
    let owner = owner(&run).unwrap();
    next["revision"] = json!(2);
    let mut admission = crate::subteam::admit(&run, &mut next, true, &owner).unwrap();
    next["status"] = json!("running");
    save_json(&run.join("manifest.json"), &next).unwrap();
    admission.published();
    // Later loss of metadata must not re-arm setup rollback or rewrite the tree.
    fs::write(run.join("manifest.json"), b"unreadable later state").unwrap();
    let admitted = read_json(&run.join("tree.json")).unwrap();
    admission.undo_unpublished().unwrap();
    assert_eq!(read_json(&run.join("tree.json")).unwrap(), admitted);
}

#[test]
fn failed_admission_save_keeps_prior_tree_and_unowned_temporary_file() {
    let (_dir, run, mut next, tree) = resume_fixture();
    let owner = owner(&run).unwrap();
    next["revision"] = json!(2);
    let obstacle = run.join(format!("tree.{}.tmp", std::process::id()));
    fs::write(&obstacle, b"unrelated atomic-write temporary file").unwrap();
    assert!(crate::subteam::admit(&run, &mut next, true, &owner).is_err());
    assert_eq!(read_json(&run.join("tree.json")).unwrap(), tree);
    assert_eq!(
        fs::read(&obstacle).unwrap(),
        b"unrelated atomic-write temporary file"
    );
    fs::remove_file(obstacle).unwrap();
    crate::subteam::admit(&run, &mut next, true, &owner)
        .unwrap()
        .undo_unpublished()
        .unwrap();
    assert_eq!(read_json(&run.join("tree.json")).unwrap(), tree);
}

#[test]
fn admission_undo_does_not_repair_unknown_prior_member_or_manifest() {
    for unknown in ["member", "manifest"] {
        let (_dir, run, mut next, _) = resume_fixture();
        let owner = owner(&run).unwrap();
        if unknown == "member" {
            let mut tree = read_json(&run.join("tree.json")).unwrap();
            tree["members"]["run"]["active"] = json!(true);
            save_json(&run.join("tree.json"), &tree).unwrap();
        }
        next["revision"] = json!(2);
        let admission = crate::subteam::admit(&run, &mut next, true, &owner).unwrap();
        if unknown == "manifest" {
            fs::write(run.join("manifest.json"), b"invalid metadata").unwrap();
        }
        let admitted = read_json(&run.join("tree.json")).unwrap();
        assert!(admission.undo_unpublished().is_err(), "{unknown}");
        assert_eq!(read_json(&run.join("tree.json")).unwrap(), admitted);
    }
}

#[test]
fn admission_requires_the_matching_run_owner() {
    let (dir, run, mut next, tree) = resume_fixture();
    let other = dir.0.join("other");
    private_dir(&other).unwrap();
    let owner = owner(&other).unwrap();
    next["revision"] = json!(2);
    assert!(crate::subteam::admit(&run, &mut next, true, &owner).is_err());
    assert_eq!(read_json(&run.join("tree.json")).unwrap(), tree);
}

#[test]
fn initial_setup_failure_removes_only_own_unpublished_admission() {
    for failure in ["manifest", "allocation"] {
        let dir = TestDir::new();
        let run = dir.0.join("new-run");
        private_dir(&run).unwrap();
        fs::write(run.join("keep"), b"unrelated payload").unwrap();
        let obstacle = run.join(format!("manifest.{}.tmp", std::process::id()));
        if failure == "manifest" {
            fs::write(&obstacle, b"not our atomic write").unwrap();
        } else {
            private_dir(&run.join("revisions")).unwrap();
        }
        let owner = owner(&run).unwrap();
        let mut manifest = json!({"run_id":"new-run","parent_run_id":null,"revision":1,
            "usage":{},"revisions":[],"creator_main_session_id":"fixture-root"});
        let mut options =
            crate::tests::options(&["fixture", "--store", dir.0.to_str().unwrap()]).unwrap();
        let error = admitted_revision(&mut options, &run, &mut manifest, &owner).unwrap_err();
        assert!(!error.contains("admission retained"), "{error}");
        assert!(!run.join("manifest.json").exists());
        let tree = read_json(&run.join("tree.json")).unwrap();
        assert!(tree["members"].get("new-run").is_none());
        assert_eq!(fs::read(run.join("keep")).unwrap(), b"unrelated payload");
        if failure == "manifest" {
            assert_eq!(fs::read(obstacle).unwrap(), b"not our atomic write");
        }
    }
}

#[test]
fn initial_admission_never_undoes_unknown_or_acknowledged_publication() {
    for acknowledged in [false, true] {
        let dir = TestDir::new();
        let run = dir.0.join("new-run");
        private_dir(&run).unwrap();
        let owner = owner(&run).unwrap();
        let mut manifest = json!({"run_id":"new-run","parent_run_id":null,"revision":1});
        let mut admission = crate::subteam::admit(&run, &mut manifest, false, &owner).unwrap();
        fs::write(run.join("manifest.json"), b"unknown publication").unwrap();
        if acknowledged {
            admission.published();
            fs::remove_file(run.join("manifest.json")).unwrap();
        }
        let tree = read_json(&run.join("tree.json")).unwrap();
        admission.undo_unpublished().unwrap();
        assert_eq!(read_json(&run.join("tree.json")).unwrap(), tree);
    }
}
