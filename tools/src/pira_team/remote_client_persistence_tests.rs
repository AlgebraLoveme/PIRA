#[test]
fn distinct_client_requests_persist_simultaneously_in_one_controller() {
    use std::sync::mpsc;
    let f = Fixture::signing();
    let state = f.root.join("controller");
    let (entered, ready) = mpsc::channel();
    let envelopes = std::thread::scope(|scope| {
        let mut releases = Vec::new();
        let mut workers = Vec::new();
        for index in 0..4 {
            let mut grant = f.grant("submit");
            grant["request_id"] = json!(format!("parallel-{index}"));
            grant["nonce"] = json!(format!("{:064x}", index + 8));
            let state = state.clone();
            let authority = f.authority.clone();
            let entered = entered.clone();
            let (release, released) = mpsc::channel();
            releases.push(release);
            workers.push(scope.spawn(move || {
                persist_submission(&state, &grant, |grant| {
                    // Every client holds its persistence lock at this barrier.
                    entered.send(index).unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                    sign_with(grant, &authority)
                })
            }));
        }
        drop(entered);
        let mut simultaneous = Vec::new();
        for _ in 0..4 {
            match ready.recv_timeout(Duration::from_secs(2)) {
                Ok(index) => simultaneous.push(index),
                Err(_) => break,
            }
        }
        for release in releases {
            let _ = release.send(());
        }
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(
            simultaneous.len(),
            4,
            "distinct requests contended: {results:?}"
        );
        results
            .into_iter()
            .map(|result| result.unwrap().0)
            .collect::<Vec<_>>()
    });
    let effects = std::cell::Cell::new(0);
    for envelope in &envelopes {
        let grant = &envelope["grant"];
        assert_eq!(
            read(&state.join(format!("{}.json", text(grant, "request_id").unwrap()))).unwrap(),
            *envelope
        );
        verify(envelope, &f.anchor).unwrap();
        submit_using(envelope.clone(), grant, &f.anchor, |_| {
            effects.set(effects.get() + 1);
            Ok(())
        })
        .unwrap();
        submit_using(envelope.clone(), grant, &f.anchor, |_| {
            panic!("duplicate effect")
        })
        .unwrap();
    }
    assert_eq!(effects.get(), 4);
    assert!(!state.join("controller.lock").exists());
}

#[test]
fn same_client_request_rejects_overlap_and_changed_binding_then_reuses_exact_envelope() {
    use std::sync::mpsc;
    let f = Fixture::signing();
    let state = f.root.join("controller");
    let grant = f.grant("submit");
    let envelope = std::thread::scope(|scope| {
        let (entered, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let state_ref = &state;
        let grant_ref = &grant;
        let authority = &f.authority;
        let first = scope.spawn(move || {
            persist_submission(state_ref, grant_ref, |grant| {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(10)).unwrap();
                sign_with(grant, authority)
            })
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut changed = grant.clone();
        changed["args"] = json!(["run", "conflicting task"]);
        let conflict = persist_submission(&state, &changed, |_| panic!("overlap must not sign"));
        release.send(()).unwrap();
        let first = first.join().unwrap().unwrap().0;
        assert!(conflict.unwrap_err().contains("live owner"));
        first
    });
    for category in ["payload", "caller", "policy", "endpoint"] {
        let mut changed = grant.clone();
        match category {
            "payload" => changed["args"] = json!(["run", "different task"]),
            "caller" => {
                changed["issuer"] = json!("other-root");
                changed["context"]["caller_thread_id"] = json!("other-root");
            }
            "policy" => changed["context"]["sandbox_policy"] = json!({"type":"read-only"}),
            "endpoint" => changed["endpoint"]["host"] = json!("another-host"),
            _ => unreachable!(),
        }
        assert!(
            persist_submission(&state, &changed, |_| panic!(
                "changed binding must not sign"
            ))
            .is_err(),
            "{category}"
        );
    }
    let mut retry = grant.clone();
    retry["nonce"] = json!("9".repeat(64));
    let repeated = persist_submission(&state, &retry, |_| panic!("retained replay must not sign"))
        .unwrap()
        .0;
    assert_eq!(repeated, envelope);
    assert_eq!(read(&state.join("fixture-request.json")).unwrap(), envelope);
    let effects = std::cell::Cell::new(0);
    submit_using(envelope.clone(), &envelope["grant"], &f.anchor, |_| {
        effects.set(effects.get() + 1);
        Ok(())
    })
    .unwrap();
    submit_using(repeated.clone(), &repeated["grant"], &f.anchor, |_| {
        panic!("replay must not respawn")
    })
    .unwrap();
    assert_eq!(effects.get(), 1);
}

#[test]
fn resume_client_persistence_keeps_original_run_binding() {
    let f = Fixture::signing();
    let state = f.root.join("controller");
    let run = f.grant("submit");
    let initial = persist_submission(&state, &run, |grant| sign_with(grant, &f.authority)).unwrap();
    assert!(initial.1.is_none());
    let mut resume = run.clone();
    resume["request_id"] = json!("resume-one");
    resume["run_request"] = run["request_id"].clone();
    resume["args"] = json!(["resume"]);
    resume["cwd"] = Value::Null;
    let retained =
        persist_submission(&state, &resume, |grant| sign_with(grant, &f.authority)).unwrap();
    assert_eq!(retained.1, Some(run));
    assert_eq!(
        persist_submission(&state, &resume, |_| panic!("resume replay must not sign")).unwrap(),
        retained
    );
    let mut invalid = resume;
    invalid["request_id"] = json!("resume-two");
    invalid["run_request"] = json!("resume-one");
    assert!(
        persist_submission(&state, &invalid, |_| panic!(
            "non-root origin must not sign"
        ))
        .unwrap_err()
        .contains("original run submission")
    );
    assert!(!state.join("resume-two.json").exists());
}
