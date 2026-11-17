use super::*;
use crate::profile::{Execution, resolve};
use std::io::{Seek, SeekFrom, Write};

struct SessionLog(PathBuf);

impl SessionLog {
    fn new() -> Self {
        let path = crate::tests::temp_path("team-profile-regression").with_extension("jsonl");
        File::create_new(&path).unwrap();
        Self(path)
    }
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn parallel_session_logs_keep_independent_contents() {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|index| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let log = SessionLog::new();
                fs::write(&log.0, index.to_string()).unwrap();
                log
            })
        })
        .collect();
    let logs: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    for (index, log) in logs.iter().enumerate() {
        assert_eq!(fs::read_to_string(&log.0).unwrap(), index.to_string());
    }
}

fn recorded_context() -> String {
    let mut context = caller("gpt-6-sol", "medium");
    context["sandbox_policy"] = json!({"type":"danger-full-access"});
    context["permission_profile"] = json!({"type":"disabled"});
    context["file_system_sandbox_policy"] = json!({"type":"unrestricted"});
    format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"caller"}}),
        json!({"type":"turn_context","payload":context})
    )
}

fn write_padding(file: &mut File, mut bytes: u64) {
    let block = vec![b'x'; 1024 * 1024];
    while bytes > 0 {
        let size = bytes.min(block.len() as u64) as usize;
        file.write_all(&block[..size]).unwrap();
        bytes -= size as u64;
    }
}

#[test]
fn session_over_256_mib_keeps_latest_context() {
    let log = SessionLog::new();
    let mut file = File::create(&log.0).unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"session_meta","payload":{"id":"caller"}})
    )
    .unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"turn_context","payload":caller("old", "low")})
    )
    .unwrap();
    // Both many small records and near-limit unrelated records contribute real bytes.
    let small = format!(
        "{}\n",
        json!({"type":"response_item","payload":{"text":"x".repeat(1024)}})
    );
    let block = small.repeat(1024);
    for _ in 0..128 {
        file.write_all(block.as_bytes()).unwrap();
    }
    for _ in 0..9 {
        file.write_all(b"{\"type\":\"response_item\",\"payload\":{\"text\":\"")
            .unwrap();
        write_padding(&mut file, 15 * 1024 * 1024);
        file.write_all(b"\",\"approval_policy\":\"on-request\",\"sandbox_policy\":{\"type\":\"danger-full-access\"}}}\n").unwrap();
    }
    drop((small, block));
    let mut current = caller("gpt-6-sol", "medium");
    current["permission_profile"] = Value::Null;
    writeln!(file, "{}", json!({"type":"turn_context","payload":current})).unwrap();
    let bytes = file.metadata().unwrap().len();
    assert!(bytes > 256 * 1024 * 1024);
    eprintln!("synthetic_session_bytes={bytes}");
    drop(file);

    let context = read(&log.0, "caller").unwrap();
    assert_eq!(context["model"], "gpt-6-sol");
    assert_eq!(context["effort"], "medium");
    assert_eq!(context["caller_thread_id"], "caller");
    let (model, effort, sources, execution) = resolve(None, None, context).unwrap();
    assert_eq!((model.as_str(), effort.as_str()), ("gpt-6-sol", "medium"));
    assert_eq!(sources, json!({"model":"parent", "effort":"parent"}));
    assert_eq!(execution.mode, "workspace-write");
}

#[test]
fn session_snapshot_does_not_read_later_appends_or_complete_a_partial_tail() {
    let log = SessionLog::new();
    fs::write(&log.0, recorded_context()).unwrap();
    let file = File::open(&log.0).unwrap();
    let len = file.metadata().unwrap().len();
    let mut append = fs::OpenOptions::new().append(true).open(&log.0).unwrap();
    writeln!(
        append,
        "{}",
        json!({"type":"session_meta","payload":{"id":"other"}})
    )
    .unwrap();
    assert_eq!(
        read_snapshot(file, len, "caller").unwrap()["model"],
        "gpt-6-sol"
    );
    assert_eq!(
        read(&log.0, "caller").unwrap_err(),
        "parent session identity mismatch"
    );
    assert!(
        read_snapshot(File::open(&log.0).unwrap(), len - 1, "caller")
            .unwrap_err()
            .contains("incomplete record")
    );
}

#[test]
fn repeated_session_metadata_preserves_current_context() {
    let log = SessionLog::new();
    let metadata = json!({"type":"session_meta","payload":{"id":"caller"}});
    let before = recorded_context();
    fs::write(&log.0, &before).unwrap();
    let expected = read(&log.0, "caller").unwrap();
    fs::write(&log.0, format!("{before}{metadata}\n{metadata}\n")).unwrap();
    assert_eq!(read(&log.0, "caller").unwrap(), expected);

    let start = json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"T"}});
    let mut payload = caller("current", "high");
    payload["turn_id"] = json!("T");
    let context = json!({"type":"turn_context","payload":payload});
    fs::write(
        &log.0,
        format!("{metadata}\n{start}\n{metadata}\n{context}\n{metadata}\n"),
    )
    .unwrap();
    let current = read(&log.0, "caller").unwrap();
    assert_eq!(current["model"], "current");
    assert_eq!(current["effort"], "high");
    assert_eq!(
        Execution::from_context(&current).unwrap().mode,
        "workspace-write"
    );
}

#[test]
fn task_boundaries_reset_context_without_backfill() {
    let log = SessionLog::new();
    let boundary = format!(
        "{}\n",
        json!({"type":"event_msg","payload":{"type":"task_started"}})
    );
    let prefix = recorded_context() + &boundary;
    fs::write(&log.0, &prefix).unwrap();
    assert_eq!(
        read(&log.0, "caller").unwrap_err(),
        "parent session has no recorded turn context"
    );

    let latest = json!({"type":"turn_context","payload":{"model":"latest"}});
    fs::write(&log.0, format!("{prefix}{latest}\n")).unwrap();
    let context = read(&log.0, "caller").unwrap();
    assert_eq!(context["model"], "latest");
    assert_eq!(context["caller_thread_id"], "caller");
    for field in [
        "effort",
        "cwd",
        "approval_policy",
        "sandbox_policy",
        "permission_profile",
        "file_system_sandbox_policy",
    ] {
        assert!(context[field].is_null(), "{field}");
    }
    assert!(resolve(Some("explicit".into()), Some("high".into()), context).is_err());

    // Contexts before identity verification cannot supply the current settings.
    let metadata = json!({"type":"session_meta","payload":{"id":"caller"}});
    fs::write(&log.0, format!("{latest}\n{metadata}\n")).unwrap();
    assert_eq!(
        read(&log.0, "caller").unwrap_err(),
        "parent session has no recorded turn context"
    );
    fs::write(&log.0, format!("{latest}\n")).unwrap();
    assert_eq!(
        read(&log.0, "caller").unwrap_err(),
        "parent session identity was not verified"
    );
}

#[test]
fn task_identity_requires_fresh_matching_context_even_for_repeated_turn_id() {
    let log = SessionLog::new();
    let metadata = json!({"type":"session_meta","payload":{"id":"caller"}});
    for task_id in [Value::Null, json!("T")] {
        let start = json!({"type":"event_msg","payload":{"type":"task_started","turn_id":task_id}});
        for context_id in [Value::Null, json!("T"), json!("other")] {
            let mut payload = caller("new-model", "high");
            payload["turn_id"] = context_id.clone();
            let context = json!({"type":"turn_context","payload":payload});
            // A boundary before the first session_meta still constrains later contexts.
            for prefix in [
                format!("{start}\n{metadata}\n"),
                recorded_context() + &format!("{start}\n"),
            ] {
                fs::write(&log.0, format!("{prefix}{context}\n{metadata}\n")).unwrap();
                if task_id == context_id {
                    assert_eq!(read(&log.0, "caller").unwrap()["model"], "new-model");
                    // Repeated task IDs are boundaries too, unlike repeated session IDs.
                    fs::write(&log.0, format!("{prefix}{context}\n{start}\n{metadata}\n")).unwrap();
                    assert_eq!(
                        read(&log.0, "caller").unwrap_err(),
                        "parent session has no recorded turn context"
                    );
                } else {
                    // A later correct record must not hide a mismatched record.
                    assert_eq!(
                        read(&log.0, "caller").unwrap_err(),
                        "parent turn context does not match active task identity"
                    );
                    let mut correct = context.clone();
                    correct["payload"]["turn_id"] = task_id.clone();
                    fs::write(&log.0, format!("{prefix}{context}\n{correct}\n")).unwrap();
                    assert!(read(&log.0, "caller").is_err());
                }
            }
        }
    }
}

#[test]
fn malformed_turn_ids_never_reuse_older_context() {
    let log = SessionLog::new();
    for id in [json!(7), json!(""), json!({}), json!([]), json!(false)] {
        for record in [
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":id}}),
            json!({"type":"turn_context","payload":{"turn_id":id}}),
        ] {
            fs::write(&log.0, format!("{}{record}\n", recorded_context())).unwrap();
            assert_eq!(
                read(&log.0, "caller").unwrap_err(),
                "malformed parent turn identity"
            );
        }
    }
}

#[test]
fn task_context_replaces_permissions_atomically() {
    let log = SessionLog::new();
    let start = json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"new"}});
    let prefix = recorded_context() + &format!("{start}\n");
    let mut current = caller("new-model", "high");
    current["turn_id"] = json!("new");
    let context_record = |payload: Value| json!({"type":"turn_context","payload":payload});
    fs::write(
        &log.0,
        format!("{prefix}{}\n", context_record(current.clone())),
    )
    .unwrap();
    let loaded = read(&log.0, "caller").unwrap();
    let execution = Execution::from_context(&loaded).unwrap();
    assert_eq!(execution.mode, "workspace-write");
    assert_eq!(execution.sandbox["networkAccess"], false);
    assert_eq!(execution.sandbox["excludeTmpdirEnvVar"], true);
    assert_eq!(execution.sandbox["excludeSlashTmp"], true);
    // Optional legacy fields absent in this context must not inherit old full-access values.
    assert!(loaded["permission_profile"].is_null());
    assert!(loaded["file_system_sandbox_policy"].is_null());
    for field in ["approval_policy", "sandbox_policy", "cwd"] {
        let mut partial = current.clone();
        partial.as_object_mut().unwrap().remove(field);
        fs::write(&log.0, format!("{prefix}{}\n", context_record(partial))).unwrap();
        let loaded = read(&log.0, "caller").unwrap();
        assert!(loaded[field].is_null(), "{field}");
        assert!(
            resolve(Some("explicit".into()), Some("high".into()), loaded).is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("approval_policy", json!("on-request")),
        ("sandbox_policy", json!({"type":"read-only"})),
        ("permission_profile", json!({"type":"restricted"})),
        ("file_system_sandbox_policy", json!({"type":"restricted"})),
    ] {
        let mut changed = current.clone();
        changed[field] = value;
        fs::write(&log.0, format!("{prefix}{}\n", context_record(changed))).unwrap();
        assert!(
            Execution::from_context(&read(&log.0, "caller").unwrap()).is_err(),
            "{field}"
        );
    }
}

#[test]
fn task_context_must_be_fully_flushed_before_use() {
    let log = SessionLog::new();
    let start = json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"next"}});
    let prefix = recorded_context() + &format!("{start}\n");
    let mut payload = caller("new-model", "high");
    payload["turn_id"] = json!("next");
    let context = json!({"type":"turn_context","payload":payload}).to_string();
    for tail in [&context[..context.len() / 2], context.as_str()] {
        fs::write(&log.0, format!("{prefix}{tail}")).unwrap();
        assert!(
            read(&log.0, "caller")
                .unwrap_err()
                .contains("incomplete record")
        );
    }
    fs::write(&log.0, format!("{prefix}{context}\n")).unwrap();
    assert_eq!(read(&log.0, "caller").unwrap()["model"], "new-model");
}

#[test]
fn session_invalid_identity_or_tail_never_falls_back_to_older_context() {
    let log = SessionLog::new();
    let prefix = recorded_context();
    for payload in [
        json!({"id":"other"}),
        json!({}),
        json!({"id":7}),
        Value::Null,
    ] {
        // Even a subsequent matching identity cannot repair a mismatched boundary.
        let invalid = json!({"type":"session_meta","payload":payload});
        fs::write(&log.0, format!("{prefix}{invalid}\n{prefix}")).unwrap();
        assert_eq!(
            read(&log.0, "caller").unwrap_err(),
            "parent session identity mismatch"
        );
    }
    for (tail, error) in [
        (
            b"{bad json}\n".as_slice(),
            "malformed parent session record",
        ),
        (
            b"{\"type\":\"response_item\",\"payload\":\"\xff\"}\n",
            "malformed parent session record",
        ),
        (b"\n", "malformed parent session record"),
        (b"{\"type\":\"response_item\"}", "incomplete record"),
        (
            b"{\"type\":\"turn_context\",\"payload\":",
            "incomplete record",
        ),
        (
            b"{\"type\":\"session_meta\",\"payload\":",
            "incomplete record",
        ),
        (
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"",
            "incomplete record",
        ),
    ] {
        let mut file = File::create(&log.0).unwrap();
        file.write_all(prefix.as_bytes()).unwrap();
        file.write_all(tail).unwrap();
        assert!(
            read(&log.0, "caller").unwrap_err().contains(error),
            "{tail:?}"
        );
    }
}

#[test]
fn session_record_limit_includes_newline_and_remains_fail_closed() {
    let log = SessionLog::new();
    let mut file = File::create(&log.0).unwrap();
    file.write_all(recorded_context().as_bytes()).unwrap();
    let start = b"{\"type\":\"response_item\",\"payload\":\"";
    let end = b"\"}\n";
    file.write_all(start).unwrap();
    write_padding(
        &mut file,
        MAX_RECORD_BYTES - (start.len() + end.len()) as u64,
    );
    file.write_all(end).unwrap();
    assert_eq!(read(&log.0, "caller").unwrap()["model"], "gpt-6-sol");
    file.seek(SeekFrom::End(-1)).unwrap();
    file.write_all(b" \n").unwrap();
    assert_eq!(
        read(&log.0, "caller").unwrap_err(),
        "parent record exceeds 16 MiB"
    );
}

fn caller(model: &str, effort: &str) -> Value {
    let mut context = restricted();
    context["model"] = json!(model);
    context["effort"] = json!(effort);
    context
}

fn restricted() -> Value {
    json!({"approval_policy":"never", "cwd":std::env::current_dir().unwrap(),
            "sandbox_policy":{"type":"workspace-write","writable_roots":[],
                "network_access":false,"exclude_tmpdir_env_var":true,"exclude_slash_tmp":true}})
}

#[test]
fn latest_verified_context_replaces_full_access_without_backfilling_resume() {
    let path = std::env::temp_dir().join(format!(
        "team-profile-{}-{}.jsonl",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let records = |latest: Value| {
        format!(
            "{}\n{}\n{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":"caller"}}),
            json!({"type":"turn_context","payload":{"approval_policy":"never","sandbox_policy":{"type":"danger-full-access"}}}),
            json!({"type":"response_item","payload":{"sandbox_policy":{"type":"danger-full-access"},"text":"not settings"}}),
            json!({"type":"turn_context","payload":latest})
        )
    };
    fs::write(&path, records(restricted())).unwrap();
    let context = read(&path, "caller").unwrap();
    assert!(read(&path, "unrelated").is_err());
    let (model, effort, sources, execution) =
        resolve(Some("retained-model".into()), Some("high".into()), context).unwrap();
    assert_eq!(
        (model.as_str(), effort.as_str()),
        ("retained-model", "high")
    );
    assert_eq!(sources, json!({"model":"explicit", "effort":"explicit"}));
    assert_eq!(execution.mode, "workspace-write");
    assert_eq!(execution.caller_thread_id, "caller");
    fs::write(&path, records(json!({"model":"current","effort":"high"}))).unwrap();
    assert!(Execution::from_context(&read(&path, "caller").unwrap()).is_err());
    fs::remove_file(path).unwrap();
}
