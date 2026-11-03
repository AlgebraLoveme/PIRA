//! Claude stream-json lifecycle; shared process supervision and control endpoint.
use super::*;
use crate::claude;

pub(crate) fn turn(
    options: &Options,
    run: &Path,
    dir: &Path,
    task: &str,
    manifest: &mut Value,
    handoff: &Path,
    guidance: (bool, bool, bool, bool),
) -> Result<Turn, String> {
    options.execution.require_writable(handoff)?;
    let roots = crate::build_roots::configured()?;
    for root in &roots {
        options.execution.require_writable(root)?;
    }
    manifest["build_roots"] = json!(roots);
    if manifest["thread_id"].is_null() {
        manifest["thread_id"] = json!(claude::uuid()?);
    }
    let session = manifest["thread_id"]
        .as_str()
        .ok_or("invalid Claude session ID")?
        .to_owned();
    let turn = claude::uuid()?;
    lifecycle::save_json(&run.join("manifest.json"), manifest)?;
    let (sender, events) = mpsc::sync_channel(64);
    let (control_sender, controls_rx) = mpsc::sync_channel(PENDING_CONTROLS + 1);
    let mut controls = create(&dir.join("controls.jsonl"))?;
    let endpoint = Endpoint::start(
        run,
        manifest,
        &options.execution.sandbox,
        control_sender,
        controls.try_clone().map_err(|e| e.to_string())?,
        if options.allow_subteam {
            Some(crate::authentication::Server::start(
                run,
                &manifest["revision"],
                &options.authentication,
            )?)
        } else {
            None
        },
    )?;
    let mut server = Server::new(
        options,
        run,
        dir,
        handoff,
        endpoint.attempt_gate.clone(),
        sender,
        events,
        controls_rx,
    )?;
    crate::subteam::bind(run, &manifest["revision"], &server.scope, dir)?;
    let baseline = manifest["usage"].clone();
    let repair = manifest["repairs"].as_u64().unwrap_or(0) > 0;
    let mut stopping: Option<Instant> = None;
    let mut initialized = false;
    let mut started = false;
    let mut idle = false;
    let mut completed_result = false;
    let mut native_failure = false;
    let mut pending_steer: Option<(String, String, String, mpsc::Sender<Value>)> = None;
    let mut pending_interrupt: Option<(String, mpsc::Sender<Value>)> = None;
    let mut text = None;
    let mut usage = None;
    let outcome = (|| -> Result<(String, Option<String>), String> {
        server.send(claude::request("initialize", "initialize"))?;
        let startup_deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if crate::CANCELLED.load(Ordering::SeqCst) && stopping.is_none() {
                stopping = Some(Instant::now() + Duration::from_secs(5));
                let _ = server.send(claude::request("cancel", "interrupt"));
            }
            if stopping.is_some_and(|deadline| Instant::now() >= deadline) {
                server.scope.terminate();
                return Ok((
                    "interrupted".into(),
                    Some(
                        "Claude cancellation grace expired; owned process scope terminated".into(),
                    ),
                ));
            }
            if !initialized && Instant::now() >= startup_deadline {
                return Err("Claude native initialization timed out".into());
            }
            if completed_result && idle && pending_steer.is_none() {
                return Ok(("completed".into(), None));
            }
            let event = if let Ok(event) = server.controls.try_recv() {
                event
            } else {
                match server.events.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => event,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(error) = server.fault.lock().unwrap().clone() {
                            native_failure = true;
                            return Err(error);
                        }
                        continue;
                    }
                    Err(_) => return Err("Claude event channel closed".into()),
                }
            };
            match event {
                Event::Closed(error) | Event::TierWriteFailed(_, error) => {
                    native_failure = true;
                    return Err(error);
                }
                Event::Control(request, reply) => {
                    let operation = request["operation"].as_str().unwrap_or("");
                    if !started || stopping.is_some() || request["turn_id"] != turn {
                        let _ = reply.send(
                            json!({"status":"rejected","error":"no matching active Claude turn"}),
                        );
                        continue;
                    }
                    if operation == "interrupt" {
                        let id = claude::uuid()?;
                        server.send(claude::request(&id, "interrupt"))?;
                        stopping = Some(Instant::now() + Duration::from_secs(5));
                        pending_interrupt = Some((id, reply));
                    } else if operation == "steer" && !repair && pending_steer.is_none() {
                        let task = request["task"].as_str().filter(|s| !s.trim().is_empty());
                        let gate = request["completion_gate"]
                            .as_str()
                            .filter(|s| !s.trim().is_empty() && !s.contains('\0'));
                        if let (Some(task), Some(gate)) = (task, gate) {
                            let id = claude::uuid()?;
                            let message = format!("{task}\n\nReplacement completion gate: {gate}");
                            server.attempt_gate.clone().with_open(|| {
                                server.send(claude::user_message(&id, &session, &message))
                            })?;
                            pending_steer = Some((id, task.into(), gate.into(), reply));
                            completed_result = false;
                            idle = false;
                        } else {
                            let _=reply.send(json!({"status":"rejected","error":"nonempty task and completion gate required"}));
                        }
                    } else {
                        let _=reply.send(json!({"status":"rejected","error":"unsupported Claude control or steer pending; settings unchanged"}));
                    }
                }
                Event::Protocol(value, _) => {
                    if value["type"] == "control_request" {
                        server.send(json!({"type":"control_response","response":{"subtype":"error","request_id":value["request_id"],"error":"unexpected request refused by Team"}}))?;
                        return Err("unexpected Claude approval/tool request refused".into());
                    }
                    if value["type"] == "control_response" {
                        let response = &value["response"];
                        if response["request_id"] == "initialize" {
                            if initialized || response["subtype"] != "success" {
                                return Err("Claude initialization rejected or repeated".into());
                            }
                            initialized = true;
                            server.attempt_gate.clone().with_open(|| {
                                server.send(claude::user_message(&turn, &session, task))
                            })?;
                        } else if pending_interrupt
                            .as_ref()
                            .is_some_and(|(id, _)| response["request_id"] == *id)
                        {
                            let (_, reply) = pending_interrupt.take().unwrap();
                            let status = if response["subtype"] == "success" {
                                "accepted"
                            } else {
                                "rejected"
                            };
                            let receipt = json!({"status":status,"operation":"interrupt","turn_id":turn,"revision":manifest["revision"]});
                            writeln!(controls, "{receipt}").map_err(|e| e.to_string())?;
                            let _ = reply.send(receipt);
                        }
                        continue;
                    }
                    if value["type"] == "system" && value["subtype"] == "session_state_changed" {
                        if value["session_id"].as_str().is_some_and(|id| id != session) {
                            return Err("Claude state belongs to a different session".into());
                        }
                        idle = value["state"] == "idle";
                        if !idle {
                            completed_result = false;
                        }
                    } else if value["type"] == "system" && value["subtype"] == "init" {
                        if !initialized
                            || value["session_id"] != session
                            || value["model"] != options.model
                            || value["permissionMode"] != "bypassPermissions"
                        {
                            return Err(
                                "Claude did not confirm session, model, and permissions".into()
                            );
                        }
                        if value["tools"].as_array().is_none_or(|tools| {
                            tools.iter().any(|tool| {
                                !["Bash", "Read", "Write", "Edit", "Glob", "Grep"]
                                    .contains(&tool.as_str().unwrap_or(""))
                            })
                        }) {
                            return Err("Claude exposed unexpected native tools".into());
                        }
                        manifest["claude_session_started"] = json!(true);
                        manifest["active_turn"] = json!(turn);
                        manifest["worker_policy_version"] = json!(crate::WORKER_POLICY_VERSION);
                        if options.allow_subteam {
                            manifest["coordinator_policy_version"] =
                                json!(crate::COORDINATOR_POLICY_VERSION);
                        }
                        for (key, flag) in [
                            ("inject_review", guidance.0),
                            ("inject_implement", guidance.1),
                            ("inject_fig", guidance.2),
                            ("inject_write", guidance.3),
                        ] {
                            manifest[key] = json!(flag);
                        }
                        lifecycle::save_json(&run.join("manifest.json"), manifest)?;
                        endpoint.activate(&turn)?;
                        started = true;
                        crate::notice(format_args!(
                            "pira_team active: {} (Claude; revision {})",
                            manifest["run_id"].as_str().unwrap_or("?"),
                            manifest["revision"]
                        ));
                    } else if value["type"] == "user" {
                        if pending_steer.as_ref().is_some_and(|(id, _, _, _)| {
                            value["uuid"] == *id && value["session_id"] == session
                        }) {
                            let (_, task, gate, reply) = pending_steer.take().unwrap();
                            manifest["task"] = json!(task);
                            manifest["completion_gate"] = json!(gate);
                            lifecycle::save_json(&run.join("manifest.json"), manifest)?;
                            let receipt = json!({"status":"accepted","operation":"steer","turn_id":turn,"revision":manifest["revision"]});
                            writeln!(controls, "{receipt}").map_err(|e| e.to_string())?;
                            let _ = reply.send(receipt);
                        }
                    } else if value["type"] == "result" {
                        if value["session_id"] != session {
                            return Err("Claude result has a different session".into());
                        }
                        usage = claude::usage(&value["usage"], &baseline);
                        text = value["result"].as_str().map(str::to_owned);
                        if stopping.is_some() {
                            return Ok(("interrupted".into(), None));
                        }
                        if value["is_error"] != false || value["subtype"] != "success" {
                            return Ok(("failed".into(), Some(claude::result_error(&value))));
                        }
                        if !started {
                            return Err("Claude result lacks verified initialization".into());
                        }
                        completed_result = true;
                    }
                }
            }
        }
    })();
    if let Some((_, _, _, reply)) = pending_steer.take() {
        let _=reply.send(json!({"status":"unknown","error":"Claude stopped before steer acknowledgement; inspect retained events"}));
    }
    if let Some((_, reply)) = pending_interrupt.take() {
        let _ = reply.send(
            json!({"status":"unknown","error":"Claude stopped before interrupt acknowledgement"}),
        );
    }
    drop(endpoint);
    server.shutdown();
    let (status, error) = match outcome {
        Ok(result) => result,
        Err(error) => (
            if stopping.is_some() {
                "interrupted"
            } else {
                "failed"
            }
            .into(),
            Some(if native_failure {
                claude::native_stderr(dir).unwrap_or(error)
            } else {
                error
            }),
        ),
    };
    Ok(Turn {
        status,
        text,
        usage,
        error,
    })
}
