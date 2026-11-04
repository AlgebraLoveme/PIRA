//! Codex native app-server turn protocol.
use super::*;

pub(crate) fn turn(
    options: &Options,
    run: &Path,
    dir: &Path,
    task: &str,
    manifest: &mut Value,
    handoff: &Path,
    guidance: (bool, bool, bool, bool),
) -> Result<Turn, String> {
    let build_roots = crate::build_roots::configured()?;
    for root in &build_roots {
        options.execution.require_writable(root)?;
    }
    options.execution.require_writable(handoff)?;
    manifest["build_roots"] = json!(build_roots);
    let repair = manifest["repairs"].as_u64().ok_or("missing repair state")? > 0;
    let (mut controls, endpoint, mut server) = start_attempt(options, run, dir, manifest, handoff)?;
    let mut stopping: Option<(&str, Instant)> = None;
    let outcome = (|| -> Result<(String, Option<String>), String> {
        server.request("initialize", json!({"clientInfo":{"name":"pira_team","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))
        .map_err(|e| if crate::CANCELLED.load(Ordering::SeqCst) { e } else { format!("native Codex strict-config stdio initialization failed: {e}; inspect stderr.log and update Codex/Team to compatible builds. This is separate from the interactive shared daemon") })?;
        server.send(json!({"method":"initialized"}))?;
        let policy = fs::read_to_string(dir.join("policy.md")).map_err(|e| e.to_string())?;
        let mut params = json!({"model":options.model,"cwd":options.cwd,"sandbox":options.execution.mode,"config":options.execution.config(),
        "approvalPolicy":"never","baseInstructions":policy,"developerInstructions":""});
        let thread = if let Some(id) = manifest["thread_id"].as_str() {
            params["threadId"] = json!(id);
            let result = server.request("thread/resume", params)?;
            if result["thread"]["id"].as_str() != Some(id) {
                return Err("Codex resumed a different thread".into());
            }
            result
        } else {
            params["ephemeral"] = json!(false);
            server.request("thread/start", params)?
        };
        if thread["sandbox"] != options.execution.sandbox || thread["approvalPolicy"] != "never" {
            return Err(format!(
                "Codex did not confirm {}/approval-never exact caller permissions",
                options.execution.mode
            ));
        }
        let thread_id = thread["thread"]["id"]
            .as_str()
            .ok_or("missing Codex thread ID")?
            .to_owned();
        server.thread_id = Some(thread_id.clone());
        manifest["thread_id"] = json!(thread_id);
        lifecycle::save_json(&run.join("manifest.json"), manifest)?;
        let phase = fs::read_to_string(dir.join("phase.md")).map_err(|e| e.to_string())?;
        if manifest["phase_instructions"].as_str() != Some(phase.as_str()) {
            server.request(
            "thread/inject_items",
            json!({"threadId":thread_id,"items":[{
                "type":"message","role":"developer","content":[{"type":"input_text","text":phase}]
            }]}),
        )?;
            manifest["phase_instructions"] = json!(phase);
            lifecycle::save_json(&run.join("manifest.json"), manifest)?;
        }
        // Persist acknowledged guidance even if the subsequent turn is interrupted or fails.
        acknowledge_guidance(options, manifest, guidance);
        lifecycle::save_json(&run.join("manifest.json"), manifest)?;
        let tier = crate::service_tier::Tier::retained(manifest)?;
        let started = server.request(
            "turn/start",
            json!({"threadId":thread_id,"input":[{"type":"text","text":task}],
        "model":options.model,"effort":options.effort,"cwd":options.cwd,"approvalPolicy":"never",
        "sandboxPolicy":options.execution.sandbox,"serviceTierForTurn":tier.backend()}),
        )?;
        let turn_id = started["turn"]["id"]
            .as_str()
            .ok_or("missing Codex turn ID")?
            .to_owned();
        server.turn_id = Some(turn_id.clone());
        manifest["active_turn"] = json!(turn_id);
        lifecycle::save_json(&run.join("manifest.json"), manifest)?;
        endpoint.activate(&turn_id)?;
        crate::notice(format_args!(
            "pira_team active: {} (revision {}; repair={repair})",
            manifest["run_id"].as_str().unwrap_or("?"),
            manifest["revision"]
        ));

        let mut pending_controls: HashMap<u64, (String, String, String, mpsc::Sender<Value>)> =
            HashMap::new();
        let mut pending_tier: Option<(u64, crate::service_tier::Tier, mpsc::Sender<Value>)> = None;
        loop {
            if stopping.is_none() && crate::CANCELLED.load(Ordering::SeqCst) {
                let status = "interrupted";
                let _ = server.send_request(
                    "turn/interrupt",
                    json!({"threadId":thread_id,"turnId":turn_id}),
                );
                stopping = Some((status, Instant::now() + Duration::from_secs(5)));
            }
            if let Some((status, deadline)) = stopping
                && Instant::now() >= deadline
            {
                server.scope.terminate();
                return Ok((status.into(), Some("cancellation did not settle; owned backend process scope terminated; detached/breakaway descendants excluded".into())));
            }
            let event = if let Ok(control) = server.controls.try_recv() {
                control
            } else if let Some(event) = logged_event(&mut server.event_log, &mut server.pending)? {
                event
            } else {
                match server.events.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => event,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(error) = server.fault.lock().unwrap().clone() {
                            Event::Closed(error)
                        } else {
                            continue;
                        }
                    }
                    Err(_) => return Err("app-server event channel closed".into()),
                }
            };
            match event {
                Event::Closed(error) => {
                    if let Some((status, _)) = stopping {
                        return Ok((status.into(), Some(error)));
                    }
                    return Ok(("failed".into(), Some(error)));
                }
                Event::TierWriteFailed(id, error) => {
                    // No more writes are safe, but the reader/watchdog still
                    // distinguish surviving work from a true backend/output fault.
                    server.writer.take();
                    if pending_tier.as_ref().is_some_and(|pending| pending.0 == id) {
                        let (_, _, reply) = pending_tier.take().unwrap();
                        let mut response = json!({"status":"unknown","publication":"unknown",
                            "error":format!("tier delivery outcome unknown; stdin closed: {error}; backend output remains monitored; do not blindly retry"),
                            "actual_service_tier":null});
                        if let Err(error) = writeln!(
                            controls,
                            "{}",
                            json!({"request_id":id,"operation":"tier","response":response})
                        ) {
                            response["diagnostic_warning"] =
                                json!(format!("tier acknowledgement log failed: {error}"));
                        }
                        let _ = reply.send(response);
                    }
                }
                Event::Control(request, reply) => {
                    let op = request["operation"].as_str().unwrap_or("");
                    if op == "tier" {
                        let update = crate::service_tier::active_update(
                            &request,
                            &thread_id,
                            &turn_id,
                            stopping.is_some(),
                            pending_tier.is_some(),
                        );
                        match update {
                            Err(error) => {
                                let _ = reply.send(json!({"status":"rejected","error":error}));
                            }
                            Ok((tier, params)) => match server
                                .send_request("turn/settings/update", params)
                            {
                                Ok(id) => {
                                    pending_tier = Some((id, tier, reply));
                                }
                                Err(error) => {
                                    let _ = reply.send(json!({"status":"rejected","error":error}));
                                }
                            },
                        }
                        continue;
                    }
                    let task = request["task"].as_str().unwrap_or("");
                    let gate = request["completion_gate"].as_str().unwrap_or("");
                    if request["turn_id"] != turn_id
                        || stopping.is_some()
                        || !["steer", "interrupt"].contains(&op)
                        || (op == "steer"
                            && (repair
                                || task.trim().is_empty()
                                || gate.trim().is_empty()
                                || gate.contains('\0')))
                    {
                        let _ = reply.send(json!({"status":"rejected","error":"stale/inactive turn, or steering unavailable during repair"}));
                        continue;
                    }
                    let (method, params) = if op == "steer" {
                        (
                            "turn/steer",
                            json!({"threadId":thread_id,"expectedTurnId":turn_id,"input":[{"type":"text","text":format!("{task}\n\nReplacement completion gate: {gate}")}]}),
                        )
                    } else {
                        (
                            "turn/interrupt",
                            json!({"threadId":thread_id,"turnId":turn_id}),
                        )
                    };
                    if op == "interrupt" {
                        stopping = Some(("interrupted", Instant::now() + Duration::from_secs(5)));
                    }
                    let id = match server.send_request(method, params) {
                        Ok(id) => id,
                        Err(error) => {
                            let _ = reply.send(json!({"status":"rejected","error":error}));
                            return Ok((stopping.map_or("failed", |s| s.0).into(), Some(error)));
                        }
                    };
                    writeln!(controls,"{}",json!({"request_id":id,"operation":op,"task":task,"turn_id":turn_id,"status":"sent"})).map_err(|e|e.to_string())?;
                    pending_controls.insert(id, (op.into(), task.into(), gate.into(), reply));
                }
                Event::Protocol(value, _) => {
                    server.observe(&value);
                    if value.get("method").is_some() && value.get("id").is_some() {
                        server.send(json!({"id":value["id"],"error":{"code":-32601,"message":"unexpected request refused by coding-worker launcher"}}))?;
                        return Err("unexpected approval/tool request refused".into());
                    }
                    if let Some(id) = value["id"].as_u64() {
                        if pending_tier.as_ref().is_some_and(|pending| pending.0 == id) {
                            let (_, tier, reply) = pending_tier.take().unwrap();
                            let mut response = crate::service_tier::publication(
                                tier,
                                &turn_id,
                                &value,
                                manifest,
                                |state| lifecycle::save_json(&run.join("manifest.json"), state),
                            );
                            // A tier-control diagnostic failure must not terminate model work.
                            if let Err(error) = writeln!(
                                controls,
                                "{}",
                                json!({"request_id":id,"operation":"tier","response":response})
                            ) {
                                response["diagnostic_warning"] =
                                    json!(format!("tier acknowledgement log failed: {error}"));
                            }
                            let _ = reply.send(response);
                            continue;
                        }
                        if let Some((op, task, gate, reply)) = pending_controls.remove(&id) {
                            let response = if let Some(error) = value.get("error") {
                                json!({"status":"rejected","error":error.to_string()})
                            } else if value.get("result").is_none() {
                                json!({"status":"rejected","error":"missing control acknowledgement"})
                            } else if op == "steer" && value["result"]["turnId"] != turn_id {
                                json!({"status":"rejected","error":"steer acknowledgement has wrong turn ID"})
                            } else {
                                if op == "steer" {
                                    manifest["task"] = json!(task);
                                    manifest["completion_gate"] = json!(gate);
                                    lifecycle::save_json(&run.join("manifest.json"), manifest)?;
                                }
                                if op == "interrupt" {
                                    stopping = Some((
                                        "interrupted",
                                        Instant::now() + Duration::from_secs(5),
                                    ));
                                }
                                json!({"status":"accepted","operation":op,"turn_id":turn_id,"revision":manifest["revision"]})
                            };
                            writeln!(controls, "{}", json!({"request_id":id,"response":response}))
                                .map_err(|e| e.to_string())?;
                            let _ = reply.send(response);
                        }
                        continue;
                    }
                    let p = &value["params"];
                    if p["threadId"] != thread_id
                        || p["turnId"].as_str().is_some_and(|id| id != turn_id)
                    {
                        continue;
                    }
                    match value["method"].as_str() {
                        Some("turn/completed") if p["turn"]["id"] == turn_id => {
                            let native = p["turn"]["status"].as_str().unwrap_or("failed");
                            let status = stopping.map(|s| s.0).unwrap_or(native).to_owned();
                            return Ok((
                                status,
                                (!p["turn"]["error"].is_null())
                                    .then(|| p["turn"]["error"].to_string()),
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
    })();
    drop(endpoint);
    // Quiesce the transport before harvesting failure evidence: a startup
    // interruption can race notifications already emitted before the start ACK.
    server.shutdown();
    let (status, mut error) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            let status = stopping.map(|s| s.0).unwrap_or_else(|| {
                if crate::CANCELLED.load(Ordering::SeqCst) {
                    "interrupted"
                } else {
                    "failed"
                }
            });
            (status.to_owned(), Some(error))
        }
    };
    if error.is_some()
        && let Err(retain_error) = server.retain_pending_evidence()
    {
        error = Some(format!(
            "{}; retain evidence: {retain_error}",
            error.unwrap()
        ));
    }
    Ok(Turn {
        status,
        text: server.text.take(),
        usage: server.usage.take(),
        error,
    })
}
