//! Codex native app-server turn protocol.
use super::*;

// PIRA: readiness threshold, not proof that native continuation is permanently
// suppressed. Only between turns; a running turn has no lifetime deadline.
const CONTINUATION_GRACE: Duration = Duration::from_secs(5);

pub(crate) fn turn(request: TurnRequest<'_>) -> Result<Turn, String> {
    let TurnRequest {
        options,
        run,
        dir,
        task,
        manifest,
        handoff,
        guidance,
    } = request;
    let build_roots = crate::profile::build_roots::configured(&options.execution)?;
    options.execution.require_writable(handoff)?;
    manifest["build_roots"] = json!(build_roots);
    let repair = manifest["repairs"].as_u64().ok_or("missing repair state")? > 0;
    let (mut controls, endpoint, mut server) = start_attempt(options, run, dir, manifest, handoff)?;
    let mut stopping: Option<(&str, Instant)> = None;
    let outcome = (|| -> Result<(String, Option<String>), String> {
        server.request("initialize", json!({"clientInfo":{"name":"pira_team","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))
        .map_err(|e| if crate::CANCELLED.load(Ordering::SeqCst) { e } else { format!("native Codex strict-config stdio initialization failed: {e}; inspect stderr.log and update Codex/Team to compatible builds. This is separate from the interactive shared daemon") })?;
        server.send(json!({"method":"initialized"}))?;
        let capability = crate::harness::codex::goal_capability(dir);
        let mut goal = Value::Null;
        let mut goal_engaged = false;
        let phase = fs::read_to_string(dir.join("phase.md")).map_err(|e| e.to_string())?;
        let policy = fs::read_to_string(dir.join("policy.md")).map_err(|e| e.to_string())?;
        let mut params = json!({"model":options.model,"cwd":options.cwd,"sandbox":options.execution.mode,"config":crate::harness::codex::execution_config(&options.execution),
        "approvalPolicy":"never","baseInstructions":policy,"developerInstructions":""});
        let thread = if let Some(id) = manifest["thread_id"].as_str() {
            // Query the unloaded persisted thread before resume: resume itself can
            // start an active goal. Never send both resume-owned and client kickoff.
            let probe = json!({"id":1,"method":"thread/goal/get","params":{"threadId":id}});
            if server
                .request_schema
                .as_ref()
                .is_some_and(|schema| schema.is_valid(&probe))
            {
                goal = match server.request("thread/goal/get", json!({"threadId":id}))
                    .and_then(|snapshot| {
                        // A notification received during the read makes its
                        // snapshot ambiguous. Do not admit from that stale view.
                        let mut pending = server.pending.clone();
                        while let Some(Event::Protocol(event, _)) =
                            logged_event(&mut server.event_log, &mut pending)?
                        {
                            if event["params"]["threadId"] == id
                                && matches!(event["method"].as_str(),
                                    Some("thread/goal/updated" | "thread/goal/cleared"))
                            {
                                return Err("native Goal changed during pre-resume state read; no model work started; retry taskless resume".into());
                            }
                        }
                        goal_snapshot(&snapshot, id)
                    })
                {
                    Ok(goal) => goal,
                    Err(error) => {
                        restore_resume_context(run, manifest)?;
                        return Err(error);
                    }
                };
            } else if manifest["native_goal"]["status"] == "active" {
                restore_resume_context(run, manifest)?;
                return Err("unsupported Codex Goal resume: thread/goal/get unavailable; no model work started".into());
            }
            if goal["status"] == "active" {
                if manifest["goal_resume_safe"] != true || repair || capability.is_err() {
                    restore_resume_context(run, manifest)?;
                    capability.as_ref().map_err(|e| {
                        format!("unsupported Codex Goal supervision: {e}; no model work started")
                    })?;
                    return Err("active native goal cannot resume with replacement task, changed policy/profile/permissions, or another phase; native resume can execute before task injection; no model work started".into());
                }
                // Cold resume reuses retained input context, so developerInstructions
                // alone need not reach the model. Native baseInstructions is the
                // model-facing base-instruction channel (currently serialized as
                // the first developer message). Keep history and policy intact;
                // serialization order must not revive a historical contract.
                params["baseInstructions"] = json!(format!(
                    "{policy}\n\nCurrent resumed invocation: the following assignment contract supersedes retained phase/output contracts, regardless of their position in serialized history. Earlier handoff paths are historical; use the handoff_path below. Later explicit main-agent steering may still replace this contract.\n\n{phase}"
                ));
                goal_engaged = true;
                server.thread_id = Some(id.to_owned());
                server.start_pending = Some(server.received_end);
            }
            params["threadId"] = json!(id);
            params = crate::harness::codex::resume_params(
                server
                    .request_schema
                    .as_ref()
                    .ok_or("missing Codex request schema")?,
                params,
            );
            if params.get("excludeTurns").is_none() {
                crate::notice(format_args!(
                    "pira_team warning: Codex schema lacks metadata-only resume; using bounded legacy resume; update Codex to avoid full-history hydration and its 16 MiB event limit"
                ));
            }
            let result = server.request("thread/resume", params)?;
            if result["thread"]["id"].as_str() != Some(id) {
                return Err("Codex resumed a different thread".into());
            }
            result
        } else {
            params["ephemeral"] = json!(false);
            server.request("thread/start", params)?
        };
        manifest
            .as_object_mut()
            .unwrap()
            .remove("goal_resume_context");
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
        if !goal_engaged && manifest["phase_instructions"].as_str() != Some(phase.as_str()) {
            server.request(
            "thread/inject_items",
            json!({"threadId":thread_id,"items":[{
                "type":"message","role":"developer","content":[{"type":"input_text","text":phase}]
            }]}),
        )?;
            manifest["phase_instructions"] = json!(phase);
            lifecycle::save_json(&run.join("manifest.json"), manifest)?;
        }
        manifest["phase_instructions"] = json!(phase);
        manifest["native_goal"] = goal.clone();
        // Persist acknowledged guidance even if the subsequent turn is interrupted or fails.
        acknowledge_guidance(options, manifest, guidance);
        lifecycle::save_json(&run.join("manifest.json"), manifest)?;
        let mut turn_id = if goal_engaged {
            String::new()
        } else {
            let tier = crate::profile::service_tier::Tier::retained(manifest)?;
            let started = server.request(
                "turn/start",
                json!({"threadId":thread_id,"input":[{"type":"text","text":task}],
                    "model":options.model,"effort":options.effort,"cwd":options.cwd,"approvalPolicy":"never",
                    "sandboxPolicy":options.execution.sandbox,"serviceTierForTurn":tier.backend()}),
            )?;
            started["turn"]["id"]
                .as_str()
                .ok_or("missing Codex turn ID")?
                .to_owned()
        };
        let mut between = goal_engaged.then(|| Instant::now() + CONTINUATION_GRACE);
        let mut status_read = None;
        let mut idle_outcome = None;
        let mut goal_read = None;
        if !turn_id.is_empty() {
            server.turn_id = Some(turn_id.clone());
            manifest["active_turn"] = json!(turn_id);
            lifecycle::save_json(&run.join("manifest.json"), manifest)?;
            endpoint.activate(&turn_id)?;
        }
        crate::notice(format_args!(
            "pira_team active: {} (revision {}; repair={repair})",
            manifest["run_id"].as_str().unwrap_or("?"),
            manifest["revision"]
        ));

        let mut pending_controls: HashMap<
            u64,
            (String, String, String, String, mpsc::Sender<Value>),
        > = HashMap::new();
        let mut pending_tier: Option<(
            u64,
            crate::profile::service_tier::Tier,
            String,
            mpsc::Sender<Value>,
        )> = None;
        loop {
            if stopping.is_none() && crate::CANCELLED.load(Ordering::SeqCst) {
                let status = "interrupted";
                if turn_id.is_empty() {
                    return Ok((status.into(), None));
                }
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
                            // recv_timeout only expires after replay and the ready
                            // queue are drained. Read native status at the threshold;
                            // silence alone never classifies permanent suppression.
                            if let Some(outcome) = idle_outcome.take() {
                                return Ok(outcome);
                            }
                            if (between.is_some_and(|deadline| Instant::now() >= deadline)
                                || (turn_id.is_empty()
                                    && goal_engaged
                                    && between.is_some()
                                    && goal["status"] != "active"))
                                && status_read.is_none()
                                && goal_read.is_none()
                            {
                                status_read = Some(server.send_request(
                                    "thread/read",
                                    json!({"threadId":thread_id,"includeTurns":false}),
                                )?);
                            }
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
                        let (_, _, _, reply) = pending_tier.take().unwrap();
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
                        let update = crate::profile::service_tier::active_update(
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
                                    pending_tier = Some((id, tier, turn_id.clone(), reply));
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
                    pending_controls.insert(
                        id,
                        (op.into(), task.into(), gate.into(), turn_id.clone(), reply),
                    );
                }
                Event::Protocol(value, _) => {
                    if value.get("method").is_some() && value.get("id").is_some() {
                        server.send(json!({"id":value["id"],"error":{"code":-32601,"message":"unexpected request refused by coding-worker launcher"}}))?;
                        return Err("unexpected approval/tool request refused".into());
                    }
                    if let Some(id) = value["id"].as_u64() {
                        if goal_read == Some(id) {
                            goal_read = None;
                            if value.get("error").is_some() {
                                return Err(format!(
                                    "Codex Goal state read failed: {}",
                                    value["error"]
                                ));
                            }
                            goal = goal_snapshot(&value["result"], &thread_id)?;
                            manifest["native_goal"] = goal.clone();
                            lifecycle::save_json(&run.join("manifest.json"), manifest)?;
                            // Goal snapshots do not prove scheduler quiescence:
                            // a native successor's public started event can lag.
                            continue;
                        }
                        if status_read == Some(id) {
                            status_read = None;
                            if value.get("error").is_some()
                                || value["result"]["thread"]["id"] != thread_id
                            {
                                return Err(format!(
                                    "Codex continuation status read failed: {value}"
                                ));
                            }
                            // A successor may have started while this RPC was in
                            // flight. Never apply an idle read to an observed turn.
                            if !turn_id.is_empty() {
                                continue;
                            }
                            match value["result"]["thread"]["status"]["type"].as_str() {
                                Some("active") => between = None,
                                Some("idle") if goal["status"] != "active" => {
                                    idle_outcome = stopped_goal(&goal, server.text.as_deref());
                                }
                                Some("idle") => idle_outcome = Some(("incomplete".into(), Some(
                                    "native goal continuation not observed within the five-second between-turn admission grace; backend status read is idle; readiness threshold only, not proof of permanent suppression; goal unchanged; resume the same thread to continue".into()))),
                                status => idle_outcome = Some(("incomplete".into(), Some(format!(
                                    "native goal continuation unavailable: backend thread status {status:?}; goal unchanged; same-thread resume available")))),
                            }
                            continue;
                        }
                        if pending_tier.as_ref().is_some_and(|pending| pending.0 == id) {
                            let (_, tier, target, reply) = pending_tier.take().unwrap();
                            let mut response = crate::profile::service_tier::publication(
                                tier,
                                &target,
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
                        if let Some((op, task, gate, target, reply)) = pending_controls.remove(&id)
                        {
                            let response = if target != turn_id {
                                json!({"status":"unknown","error":"target turn ended before acknowledgement; control not replayed into successor"})
                            } else if let Some(error) = value.get("error") {
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
                    if p["threadId"] != thread_id {
                        continue;
                    }
                    match value["method"].as_str() {
                        Some("thread/goal/updated") => {
                            // Goal snapshots are thread state. A stopped update can
                            // precede turn/completed; never end the running turn here.
                            goal = goal_snapshot(p, &thread_id)?;
                            if goal.is_null() {
                                return Err("native Goal update requires a non-null goal; use goal/cleared for clear".into());
                            }
                            // A later notification supersedes pending reads and
                            // any tentative idle settlement based on earlier state.
                            goal_read = None;
                            status_read = None;
                            idle_outcome = None;
                            if goal["status"] == "active" {
                                capability.as_ref().map_err(|e| {
                                    format!("unsupported Codex Goal supervision: {e}")
                                })?;
                                goal_engaged = true;
                            }
                            manifest["native_goal"] = goal.clone();
                            lifecycle::save_json(&run.join("manifest.json"), manifest)?;
                        }
                        Some("thread/goal/cleared") => {
                            goal_read = None;
                            status_read = None;
                            idle_outcome = None;
                            goal = Value::Null;
                            manifest["native_goal"] = Value::Null;
                            lifecycle::save_json(&run.join("manifest.json"), manifest)?;
                        }
                        Some("turn/started") => {
                            let id = p["turn"]["id"]
                                .as_str()
                                .ok_or("missing native continuation turn ID")?;
                            if id == turn_id {
                                continue;
                            }
                            if !goal_engaged || !turn_id.is_empty() {
                                return Err(
                                    "unexpected native turn admission outside goal continuation"
                                        .into(),
                                );
                            }
                            // Positive admission is observed before any text; no
                            // duration limit applies to this running turn.
                            turn_id = id.to_owned();
                            between = None;
                            idle_outcome = None;
                            // Outstanding metadata replies describe the previous
                            // gap, not this turn or its eventual successor gap.
                            status_read = None;
                            goal_read = None;
                            server.turn_id = Some(turn_id.clone());
                            server.text = None;
                            manifest["active_turn"] = json!(turn_id);
                            lifecycle::save_json(&run.join("manifest.json"), manifest)?;
                            endpoint.activate(&turn_id)?;
                            crate::notice(format_args!("pira_team native continuation: {turn_id}"));
                        }
                        Some("turn/completed") if p["turn"]["id"] == turn_id => {
                            let native = p["turn"]["status"].as_str().unwrap_or("failed");
                            let status = stopping.map(|s| s.0).unwrap_or(native).to_owned();
                            let error = (!p["turn"]["error"].is_null())
                                .then(|| p["turn"]["error"].to_string());
                            if status != "completed" || error.is_some() || !goal_engaged {
                                return Ok((status, error));
                            }
                            // A terminal goal observed BEFORE this exact turn
                            // ended cannot launch a subsequent native goal turn.
                            if let Some(outcome) = stopped_goal(&goal, server.text.as_deref()) {
                                return Ok(outcome);
                            }
                            turn_id.clear();
                            // Empty target rejects late evidence/controls from the
                            // ended turn, while the endpoint still accepts interrupt.
                            server.turn_id = Some(String::new());
                            manifest["active_turn"] = Value::Null;
                            lifecycle::save_json(&run.join("manifest.json"), manifest)?;
                            endpoint.between_turns()?;
                            crate::notice(format_args!("pira_team awaiting native continuation"));
                            between = Some(Instant::now() + CONTINUATION_GRACE);
                            goal_read =
                                Some(server.send_request(
                                    "thread/goal/get",
                                    json!({"threadId":thread_id}),
                                )?);
                        }
                        Some("thread/tokenUsage/updated") => server.observe(&value),
                        _ if !turn_id.is_empty() => server.observe(&value),
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
    if status == "failed" {
        // Preserve native error JSON for capacity/control consumers; auth repair
        // is setup-only, including revocation that offline status cannot detect.
        crate::notice(format_args!(
            "pira_team: for authentication failures, {}",
            crate::authentication::selection::REPAIR
        ));
    }
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

fn restore_resume_context(run: &Path, manifest: &mut Value) -> Result<(), String> {
    if let Some(context) = manifest
        .as_object_mut()
        .unwrap()
        .remove("goal_resume_context")
    {
        for (key, value) in context
            .as_object()
            .ok_or("invalid retained resume context")?
        {
            manifest[key] = value.clone();
        }
        lifecycle::save_json(&run.join("manifest.json"), manifest)?;
    }
    Ok(())
}

fn goal_snapshot(result: &Value, thread: &str) -> Result<Value, String> {
    if !result.is_object() {
        return Err("missing native Goal snapshot result".into());
    }
    let goal = result
        .get("goal")
        .ok_or("missing required native Goal snapshot.goal field")?;
    if !goal.is_null()
        && (goal["threadId"] != thread
            || ![
                "active",
                "complete",
                "paused",
                "blocked",
                "usageLimited",
                "budgetLimited",
            ]
            .contains(&goal["status"].as_str().unwrap_or("")))
    {
        return Err("unsupported native Goal snapshot identity/status".into());
    }
    Ok(goal.clone())
}

fn stopped_goal(goal: &Value, candidate: Option<&str>) -> Option<(String, Option<String>)> {
    match goal["status"].as_str() {
        Some("active") => None,
        Some("complete") => Some(("completed".into(), None)),
        // Preserve the worker's blocker/incomplete handoff semantics. Lifecycle
        // still validates its file/format; a stopped goal cannot certify success.
        _ if candidate
            .and_then(|text| crate::artifact::Contract::control(text).ok())
            .is_some_and(|(status, _)| status != "completed") =>
        {
            Some(("completed".into(), None))
        }
        status => Some((
            "incomplete".into(),
            Some(format!(
                "native goal {}; current turn ended without goal completion",
                status.unwrap_or("cleared")
            )),
        )),
    }
}
