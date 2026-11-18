#[test]
fn settlement_tree_read_failure_rejects_validated_completion() {
    let (_dir, run, mut manifest, _) = resume_fixture();
    let prior = fs::read(run.join("revisions/000001/manifest.json")).unwrap();
    let handoff = run.join("handoff");
    fs::write(&handoff, "valid report").unwrap();
    let result = artifact::Contract::new("text".into(), None, None)
        .unwrap()
        .validate_file(r#"{"status":"completed","format":"text"}"#, &handoff)
        .unwrap();
    manifest["status"] = json!(result.status);
    assert_eq!(manifest["status"], "completed");
    fs::write(run.join("tree.json"), b"{").unwrap();
    let error = settle_revision(&run, &mut manifest).unwrap_err();
    assert!(error.contains("invalid run metadata"), "{error}");
    assert_eq!(manifest["status"], "failed");
    assert_eq!(manifest["error"], error);
    assert_eq!(
        fs::read(run.join("revisions/000001/manifest.json")).unwrap(),
        prior
    );
}

#[test]
fn settlement_keeps_deliberate_subtree_refusals() {
    for (blocked, expected) in [(false, "incomplete"), (true, "interrupted")] {
        let (_dir, run, mut manifest, mut tree) = resume_fixture();
        if blocked {
            tree["blocked"] = json!(true);
        } else {
            tree["members"]["child"] = json!({"active":true,"revision":1,"blocked":false});
        }
        save_json(&run.join("tree.json"), &tree).unwrap();
        assert!(settle_revision(&run, &mut manifest).is_err());
        assert_eq!(manifest["status"], expected);
        assert_eq!(
            read_json(&run.join("manifest.json")).unwrap()["status"],
            expected
        );
    }
}

#[test]
fn settlement_read_failure_preserves_non_completed_dispositions() {
    for status in ["needs_decision", "incomplete", "failed", "interrupted"] {
        let (_dir, run, mut manifest, _) = resume_fixture();
        manifest["status"] = json!(status);
        fs::write(run.join("tree.json"), b"{").unwrap();
        assert!(settle_revision(&run, &mut manifest).is_err());
        assert_eq!(manifest["status"], status);
    }
}

#[test]
fn metadata_writer_and_reader_share_exact_encoded_boundary() {
    let dir = TestDir::new();
    let path = dir.0.join("manifest.json");
    // Two JSON quotes and the persisted newline are part of the common limit.
    let at_limit = json!("x".repeat(MAX_METADATA_BYTES - 3));
    save_json(&path, &at_limit).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        MAX_METADATA_BYTES as u64
    );
    assert_eq!(read_json(&path).unwrap(), at_limit);
    let prior = fs::read(&path).unwrap();
    let over_limit = json!("x".repeat(MAX_METADATA_BYTES - 2));
    assert_eq!(
        save_json(&path, &over_limit).unwrap_err(),
        "run metadata exceeds 16 MiB"
    );
    assert_eq!(fs::read(&path).unwrap(), prior);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    fs::write(&path, vec![b' '; MAX_METADATA_BYTES + 1]).unwrap();
    assert_eq!(read_json(&path).unwrap_err(), "run metadata exceeds 16 MiB");
}
