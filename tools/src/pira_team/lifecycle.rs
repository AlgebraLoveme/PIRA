//! Durable run ownership and immutable revision outputs.
use crate::{IsolatedHome, Options, app_server, artifact, create, private_dir, profile, storage};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

pub(crate) fn decision_workspace(cwd: &Path) -> Result<PathBuf, String> {
    let cwd = cwd
        .canonicalize()
        .map_err(|e| format!("Dec workspace: {e}"))?;
    if !cwd.is_dir() {
        return Err("Dec workspace must be a directory".into());
    }
    let workspace = cwd
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(&cwd);
    if workspace.to_str().is_none() {
        return Err(
            "Team Dec workspace must be UTF-8 for retained metadata and shell configuration".into(),
        );
    }
    Ok(workspace.to_path_buf())
}

fn retained_dec_workspace(manifest: &Value) -> Result<Option<PathBuf>, String> {
    let path = match manifest.get("dec_workspace") {
        // Old runs cannot reconstruct their original caller's workspace safely.
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(path)) => PathBuf::from(path),
        Some(_) => return Err("invalid persisted dec_workspace".into()),
    };
    if !path.is_absolute() {
        return Err("persisted dec_workspace must be absolute".into());
    }
    // Windows verbatim paths can retain parent components through canonicalization.
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err("persisted dec_workspace must remain the same canonical directory".into());
    }
    let physical = path
        .canonicalize()
        .map_err(|e| format!("persisted dec_workspace: {e}"))?;
    if physical != path || !physical.is_dir() {
        return Err("persisted dec_workspace must remain the same canonical directory".into());
    }
    Ok(Some(path))
}

pub fn private_path(path: &Path, directory: bool) -> Result<(), String> {
    let meta =
        fs::symlink_metadata(path).map_err(|e| format!("inspect {}: {e}", path.display()))?;
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Err("run state must use real directories and regular files, not symlinks".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err("run state must be user-owned and not writable by group/others".into());
        }
    }
    Ok(())
}

pub fn read_json(path: &Path) -> Result<Value, String> {
    private_path(path, false)?;
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| f.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("run metadata exceeds 16 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid run metadata: {e}"))
}

pub fn save_json(path: &Path, value: &Value) -> Result<(), String> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    // Cleanup owns this path only after exclusive creation succeeds.
    let mut file = create(&temp)?;
    let result = (|| {
        writeln!(file, "{value:#}").map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temp, path).map_err(|e| e.to_string())?;
        if crate::remote::delegated_context().is_some() {
            crate::remote::sync_dir(path.parent().ok_or("missing remote metadata parent")?)?;
        }
        Ok(())
    })();
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn owner(run: &Path) -> Result<File, String> {
    private_path(run, true)?;
    private_path(run.parent().ok_or("missing store")?, true)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for parent in run.ancestors() {
            let m = fs::metadata(parent).map_err(|e| e.to_string())?;
            if (m.uid() != unsafe { libc::geteuid() } && m.uid() != 0)
                || (m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0)
            {
                return Err("unsafe run ancestor".into());
            }
        }
    }
    let path = run.join("run.lock");
    if fs::symlink_metadata(&path).is_ok() {
        private_path(&path, false)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|_| "run already has an active owner")?;
    Ok(file)
}

const FIX_TASK: &str = include_str!("fix_task.md");

pub fn launch(options: Options) -> Result<(), String> {
    for path in [
        &options.store,
        &options.cwd,
        &options.ctx_store,
        &options.dec_store,
    ] {
        options.execution.require_writable(path)?;
    }
    let run = crate::prepare_store(&options.store)?;
    launch_at(options, run)
}

// Remote requests allocate and durably bind the run before acknowledging submission.
pub(super) fn launch_at(mut options: Options, run: PathBuf) -> Result<(), String> {
    let _owner = owner(&run)?;
    let mut manifest = json!({"schema_version":4,"worker_policy_version":crate::WORKER_POLICY_VERSION,"transport":"app-server","run_id":run.file_name().unwrap().to_string_lossy(),
        "creator_main_session_id":options.execution.caller_thread_id,"parent_run_id":null,"revision":1,"thread_id":null,"usage":{},"usage_complete":true,"revisions":[],
        "sandbox":options.execution.mode,"pira_instructions":false});
    if let Some(parent) = crate::subteam::caller_run()? {
        manifest["parent_run_id"] = json!(
            parent
                .file_name()
                .unwrap()
                .to_str()
                .ok_or("non-UTF-8 coordinator ID")?
        );
    }
    manifest["allow_subteam"] = json!(options.allow_subteam);
    crate::subteam::admit(&run, &mut manifest, false)?;
    revision(&mut options, &run, &mut manifest)
}

pub fn existing(args: &[String]) -> Result<(), String> {
    let op = args[0].as_str();
    let mut root = None;
    let mut positional = Vec::new();
    let mut allow_fix = None;
    let mut inject_review = None;
    let mut inject_implement = None;
    let mut inject_fig = None;
    let mut inject_write = None;
    let mut allow_subteam = None;
    let mut recursive = false;
    let mut service_tier = None;
    let mut gate = None;
    let (mut model, mut effort, mut output, mut task_file, mut task) =
        (None, None, None, None, None);
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            positional.extend(rest.cloned());
            break;
        }
        if !arg.starts_with('-') {
            positional.push(arg.clone());
            continue;
        }
        if op != "interrupt"
            && let Some(value) = arg.strip_prefix("--task=")
        {
            crate::set_once(&mut task, value.to_owned(), "task")?;
            continue;
        }
        if op != "interrupt"
            && let Some(value) = arg.strip_prefix("--completion-gate=")
        {
            crate::set_once(&mut gate, value.to_owned(), "--completion-gate")?;
            continue;
        }
        if op == "interrupt" && matches!(arg.as_str(), "-r" | "--recursive") {
            recursive = true;
            continue;
        }
        if op == "resume" && arg == "--allow-subteam" {
            crate::set_once(&mut allow_subteam, true, "--allow-subteam")?;
            continue;
        }
        if op == "resume" && arg == "--inject-review" {
            crate::set_once(&mut inject_review, true, "--inject-review")?;
            continue;
        }
        if op == "resume" && arg == "--inject-implement" {
            crate::set_once(&mut inject_implement, true, "--inject-implement")?;
            continue;
        }
        if op == "resume" && arg == "--inject-write" {
            crate::set_once(&mut inject_write, true, "--inject-write")?;
            continue;
        }
        if op == "resume" && arg == "--inject-fig" {
            crate::set_once(&mut inject_fig, true, "--inject-fig")?;
            continue;
        }
        if op == "resume" && arg == "--allow-fix" {
            crate::set_once(&mut allow_fix, true, "--allow-fix")?;
            eprintln!(
                "pira_team: --allow-fix is deprecated; use an implementation task and completion gate"
            );
            continue;
        }
        let value = rest
            .next()
            .ok_or_else(|| format!("missing value for {arg}"))?;
        match arg.as_str() {
            "--store" => root = Some(PathBuf::from(value)),
            "--completion-gate" if op != "interrupt" => {
                crate::set_once(&mut gate, value.clone(), "--completion-gate")?
            }
            "--model" if op == "resume" => model = Some(value.clone()),
            "--effort" if op == "resume" => effort = Some(value.clone()),
            "--service-tier" if op == "resume" => crate::set_once(
                &mut service_tier,
                crate::service_tier::Tier::parse(value)?,
                "--service-tier",
            )?,
            "--timeout" if op == "resume" => {
                return Err("--timeout is no longer supported; use interrupt to stop a run".into());
            }
            "--output" if op == "resume" => output = Some(value.clone()),
            "--task" if op != "interrupt" => crate::set_once(&mut task, value.clone(), "task")?,
            "--task-file" if op != "interrupt" => {
                crate::set_once(&mut task_file, PathBuf::from(value), "--task-file")?
            }
            _ => return Err(format!("unsupported {op} option {arg}")),
        }
    }
    if positional.is_empty() || positional.len() > 2 || (op == "interrupt" && positional.len() != 1)
    {
        return Err("expected RUN_ID and, for resume/steer, --task or --task-file".into());
    }
    if op == "tier" {
        if positional.len() != 2 || task.is_some() || task_file.is_some() || gate.is_some() {
            return Err("expected tier RUN standard|fast [--store DIR]".into());
        }
        let tier = crate::service_tier::Tier::parse(&positional[1])?;
        let root = root.map(Ok).unwrap_or_else(storage::default_root)?;
        let session = profile::caller_session()?;
        let run = storage::locate_for(&root, &positional[0], &session)?;
        private_path(&run, true)?;
        let mut receipt = app_server::control(&run, "tier", tier.name(), None)?;
        receipt["run_id"] = json!(run.file_name().unwrap().to_string_lossy());
        println!("{receipt}");
        return if receipt["status"] == "accepted" {
            Ok(())
        } else {
            Err("tier not acknowledged as applied; inspect the structured receipt and original workload outcome".into())
        };
    }
    // Keep the former positional form for existing callers; never silently choose between inputs.
    if let Some(legacy) = positional.get(1) {
        crate::set_once(&mut task, legacy.clone(), "task")?;
    }
    let replacement = task.is_some() || task_file.is_some() || allow_fix.unwrap_or(false);
    if (replacement || op == "steer") && gate.is_none() {
        return Err("replacement task requires --completion-gate".into());
    }
    let gate = gate.map(|g| crate::completion_gate(Some(g))).transpose()?;
    let task = if op == "interrupt" {
        String::new()
    } else {
        crate::task_input(
            task,
            task_file,
            (op == "resume").then_some(if allow_fix.unwrap_or(false) {
                FIX_TASK
            } else {
                include_str!("continue_task.md")
            }),
        )?
    };
    let root = root.map(Ok).unwrap_or_else(storage::default_root)?;
    let session = profile::caller_session()?;
    let run = storage::locate_for(&root, &positional[0], &session)?;
    private_path(&run, true)?;
    if op != "resume" {
        let mut receipt = if op == "interrupt" {
            crate::subteam::interrupt(&run, recursive)?
        } else {
            app_server::control(&run, op, &task, gate.as_deref())?
        };
        receipt["run_id"] = json!(run.file_name().unwrap().to_string_lossy());
        println!("{receipt}");
        return Ok(());
    }
    let parent =
        profile::load().map_err(|e| format!("cannot inherit verified caller execution: {e}"))?;
    profile::Execution::from_context(&parent)?.require_writable(&run)?;
    let _owner = owner(&run)?;
    let mut manifest = read_json(&run.join("manifest.json"))?;
    // Backfill only after original creator evidence passed the ownership gate.
    manifest["creator_main_session_id"] = json!(session);
    if manifest["transport"] != "app-server" || manifest["thread_id"].as_str().is_none() {
        return Err(
            "run has no retained native conversation; legacy ephemeral runs cannot resume".into(),
        );
    }
    if manifest["status"] == "running" {
        return Err("run retains unsettled running state; refusing automatic replay; use interrupt -r RUN_ID for guarded settlement, inspect retained diagnostics and external side effects, then explicitly resume".into());
    }
    if ![
        "completed",
        "needs_decision",
        "incomplete",
        "interrupted",
        "timed_out",
        "failed",
    ]
    .contains(&manifest["status"].as_str().unwrap_or(""))
    {
        return Err("run is not in a resumable terminal state".into());
    }
    private_path(&run.join("codex-home"), true)?;
    let text = |key: &str| {
        manifest[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("missing persisted {key}"))
    };
    let sources = json!({"model":if model.is_some() {"explicit"} else {"run"},
        "effort":if effort.is_some() {"explicit"} else {"run"}});
    let (model, effort, _, execution) = profile::resolve(
        Some(model.unwrap_or(text("model")?)),
        Some(effort.unwrap_or(text("effort")?)),
        parent,
    )?;
    let output = output.unwrap_or(text("output")?);
    if !["artifact", "answer"].contains(&output.as_str()) {
        return Err("invalid output".into());
    }
    let cwd = PathBuf::from(text("cwd")?)
        .canonicalize()
        .map_err(|e| format!("working directory: {e}"))?;
    if !cwd.is_dir() {
        return Err("working directory is not a directory".into());
    }
    let completion_gate = crate::completion_gate(
        gate.or_else(|| manifest["completion_gate"].as_str().map(str::to_owned)),
    )?;
    let ctx_store = manifest["ctx_store"]
        .as_str()
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| crate::tool_store("ctx", &cwd))?;
    let dec_store = manifest["dec_store"]
        .as_str()
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| crate::tool_store("dec", &cwd))?;
    let dec_workspace = retained_dec_workspace(&manifest)?;
    for path in [&run, &cwd, &ctx_store, &dec_store] {
        execution.require_writable(path)?;
    }
    let mut options = Options {
        cwd,
        store: root,
        model,
        effort,
        profile_sources: sources,
        service_tier: service_tier
            .map(Ok)
            .unwrap_or_else(|| crate::service_tier::Tier::retained(&manifest))?,
        execution,
        task,
        label: None,
        output,
        navigation: text("navigation")?,
        completion_gate,
        inject_review: inject_review.unwrap_or(false),
        inject_implement: inject_implement.unwrap_or(false),
        inject_fig: inject_fig.unwrap_or(false),
        inject_write: inject_write.unwrap_or(false),
        allow_subteam: allow_subteam.unwrap_or(false) || manifest["allow_subteam"] == true,
        authentication: crate::authentication::Authentication::Caller,
        ctx_store,
        dec_store,
        dec_workspace,
        contract: artifact::Contract::restore(&manifest["contract"])?,
    };
    if options.allow_subteam && !manifest["parent_run_id"].is_null() {
        return Err("ordinary children cannot be granted --allow-subteam".into());
    }
    manifest["allow_subteam"] = json!(options.allow_subteam);
    manifest["schema_version"] = json!(4);
    let next = manifest["revision"]
        .as_u64()
        .filter(|n| *n < 999999)
        .ok_or("invalid or exhausted revision number")?
        + 1;
    manifest["revision"] = json!(next);
    crate::subteam::admit(&run, &mut manifest, true)?;
    revision(&mut options, &run, &mut manifest)
}

fn usage_update(manifest: &mut Value, usage: Option<Value>, baseline: &Value) {
    let Some(Value::Object(fields)) = usage else {
        manifest["usage_complete"] = json!(false);
        return;
    };
    let complete = ["input_tokens", "cached_input_tokens", "output_tokens"]
        .iter()
        .all(|key| fields.get(*key).and_then(Value::as_u64).is_some())
        && fields.iter().all(|(key, value)| {
            value
                .as_u64()
                .is_some_and(|n| n >= manifest["usage"][key].as_u64().unwrap_or(0))
        });
    if !complete {
        manifest["usage_complete"] = json!(false);
        return;
    }
    let delta = fields
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                json!(
                    value
                        .as_u64()
                        .unwrap()
                        .saturating_sub(baseline[key].as_u64().unwrap_or(0))
                ),
            )
        })
        .collect();
    manifest["usage"] = Value::Object(fields);
    manifest["revision_usage"] = Value::Object(delta);
    // Incomplete accounting stays latched even if a later report is well formed.
}

fn begin_revision(
    snapshot: &Path,
    publish: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    private_dir(snapshot).map_err(|e| e.to_string())?;
    if let Err(error) = publish() {
        // Own only this new, uncommitted allocation. Never recursively remove payload.
        return match fs::remove_dir(snapshot) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; uncommitted revision retained at {}: {cleanup}",
                snapshot.display()
            )),
        };
    }
    Ok(())
}

fn revision(options: &mut Options, run: &Path, manifest: &mut Value) -> Result<(), String> {
    let _watcher = crate::subteam::Watcher::start(run, manifest)?;
    let number = manifest["revision"].as_u64().ok_or("missing revision")?;
    let revisions = run.join("revisions");
    if number == 1 {
        private_dir(&revisions).map_err(|e| e.to_string())?;
    }
    private_path(&revisions, true)?;
    let snapshot = revisions.join(format!("{number:06}"));
    let dir = if number == 1 {
        run.to_owned()
    } else {
        snapshot.clone()
    };
    let baseline = manifest["usage"].clone();
    for key in [
        "result",
        "artifact",
        "format",
        "error",
        "active_turn",
        "validation",
    ] {
        manifest.as_object_mut().unwrap().remove(key);
    }
    for (key,value) in json!({"status":"running","model":options.model,"effort":options.effort,
        "profile_sources":options.profile_sources,"service_tier":options.service_tier.name(),"tier_publication":null,"cwd":options.cwd,"task":options.task,
        "contract":options.contract.description(),"navigation":options.navigation,"completion_gate":options.completion_gate,"ctx_store":options.ctx_store,"dec_store":options.dec_store,"dec_workspace":options.dec_workspace,"output":options.output,
        "sandbox":options.execution.mode,"execution_permissions":{"source":"verified-caller-turn-context","caller_thread_id":options.execution.caller_thread_id,"sandbox_policy":options.execution.sandbox,"approval_policy":"never"},"timeout_seconds":null,"attempts":[],"repairs":0,"revision_usage":{},"logs":dir}).as_object().unwrap() {
        manifest[key] = value.clone();
    }
    if crate::remote::delegated_context().is_some() {
        manifest["execution_permissions"]["source"] = json!("launcher-signed-delegation-v2");
    }
    begin_revision(&snapshot, || {
        save_json(&run.join("manifest.json"), manifest)
    })?;
    crate::notice(format_args!(
        "pira_team run_id: {}",
        manifest["run_id"].as_str().unwrap()
    ));
    crate::notice(format_args!("pira_team logs: {}", dir.display()));
    let start = Instant::now();
    // Attach only after a valid run has its initial durable manifest. Metadata never
    // enters worker inputs; failure follows the normal failed-run accounting path.
    let bookmark = if number == 1 {
        options
            .label
            .as_deref()
            .map(|label| {
                crate::bookmarks::assign(
                    &options.store,
                    manifest["run_id"].as_str().unwrap(),
                    label,
                )
            })
            .transpose()
    } else {
        Ok(None)
    };
    let mut outcome = bookmark.and_then(|receipt| {
        if let Some(receipt) = receipt {
            crate::notice(format_args!("pira_team bookmark: {receipt}"));
        }
        options.authentication = crate::authentication::inherit(run, manifest)?;
        generate_artifact(options, run, &dir, manifest, &baseline)
    });
    manifest["elapsed_seconds"] = json!(start.elapsed().as_secs_f64());
    manifest["active_turn"] = Value::Null;
    match &outcome {
        Ok(_) => {}
        Err(error) => {
            if manifest["status"] == "running" {
                manifest["status"] = json!(if crate::CANCELLED
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    "interrupted"
                } else {
                    "failed"
                });
            }
            manifest["error"] = json!(error);
        }
    }
    if let Err(error) = crate::subteam::finish(run, manifest) {
        outcome = Err(error);
    }
    manifest["validation"] = json!(if manifest["status"] != "completed" {
        "format only; completion not claimed"
    } else {
        "format and supplied constraints only; not factual accuracy"
    });
    let summary = json!({"revision":number,"status":manifest["status"],"artifact":manifest["artifact"],
        "usage":manifest["revision_usage"],"manifest":format!("revisions/{number:06}/manifest.json")});
    manifest["revisions"]
        .as_array_mut()
        .ok_or("invalid revisions")?
        .push(summary);
    save_json(&snapshot.join("manifest.json"), manifest)?;
    save_json(&run.join("manifest.json"), manifest)?;
    if manifest["repairs"].as_u64().unwrap_or(0) > 0 {
        eprintln!(
            "pira_team warning: format repair attempted; inspect pira_team read {} manifest.json for attempts and usage",
            manifest["run_id"].as_str().unwrap()
        );
    }
    if manifest["usage_complete"] != true {
        eprintln!(
            "pira_team warning: usage accounting incomplete; retained totals may undercount; inspect pira_team read {} manifest.json",
            manifest["run_id"].as_str().unwrap()
        );
    }
    match outcome {
        Ok(content) => {
            if options.output == "answer" {
                print!("{content}");
            } else {
                println!(
                    "{}",
                    json!({"status":manifest["status"],"run_id":manifest["run_id"],
                    "run_root":run,"handoff_path":manifest["result"]})
                );
            }
            Ok(())
        }
        Err(error) => Err(format!("{error}; logs: {}", dir.display())),
    }
}

// Keep fallible generation separate so every failure returns through revision finalization.
fn generate_artifact(
    options: &Options,
    run: &Path,
    dir: &Path,
    manifest: &mut Value,
    baseline: &Value,
) -> Result<String, String> {
    // Only a new combined launch starts staged work. Adding guidance on a retained
    // standalone run must not retroactively impose another review or unload guidance.
    if manifest["revision"] == 1 && options.inject_review && options.inject_implement {
        manifest["stage"] = json!("review");
    }
    if manifest["stage"] == "review" {
        let content = generate_stage(options, run, dir, manifest, baseline, true)?;
        if manifest["status"] != "running" {
            return Ok(content);
        }
        // The checkpoint is internal; only implementation can satisfy the final gate.
        manifest["stage"] = json!("implementation");
        save_json(&run.join("manifest.json"), manifest)?;
        let implementation = dir.join("implementation");
        private_dir(&implementation).map_err(|e| e.to_string())?;
        return generate_stage(options, run, &implementation, manifest, baseline, false);
    }
    generate_stage(options, run, dir, manifest, baseline, false)
}

// Keep assembly separate from I/O so retained additions and staged selection are testable.
fn policy_bundle(
    options: &Options,
    manifest: &Value,
    review_stage: bool,
) -> (String, (bool, bool, bool, bool)) {
    let mut common = format!("{}\n\n{}", crate::POLICY, artifact::INSTRUCTIONS);
    if options.allow_subteam {
        common.push_str("\n\n");
        common.push_str(crate::subteam::POLICY);
    } else {
        common.push_str(include_str!("ordinary_worker.md"));
    }
    // Earlier retained policies already contained both task guides. Never pretend to unload them.
    let legacy = manifest["worker_policy_version"].as_u64().unwrap_or(0) < 6;
    let had_review = legacy || manifest["inject_review"].as_bool().unwrap_or(false);
    let had_implement = legacy || manifest["inject_implement"].as_bool().unwrap_or(false);
    let review = had_review || options.inject_review || review_stage;
    let implement = !review_stage
        && (had_implement || options.inject_implement || manifest["stage"] == "implementation");
    let figure = manifest["inject_fig"].as_bool().unwrap_or(false) || options.inject_fig;
    let write = manifest["inject_write"].as_bool().unwrap_or(false) || options.inject_write;
    let mut bundle = common;
    if review {
        bundle.push_str("\n\n");
        bundle.push_str(crate::REVIEW_POLICY);
    }
    if implement {
        bundle.push_str("\n\n");
        bundle.push_str(crate::IMPLEMENTATION_POLICY);
    }
    if figure {
        bundle.push_str("\n\n");
        bundle.push_str(crate::FIGURE_POLICY);
    }
    if write {
        bundle.push_str("\n\n");
        bundle.push_str(crate::WRITING_APPLICATION);
        bundle.push_str("\n\n");
        bundle.push_str(crate::WRITING_POLICY);
    }
    (bundle, (review, implement, figure, write))
}

fn policy_additions(
    options: &Options,
    manifest: &Value,
    bundle: &str,
    (review, implement, figure, write): (bool, bool, bool, bool),
    retained_prefix: bool,
) -> String {
    let mut migration = String::new();
    if manifest["worker_policy_version"] != crate::WORKER_POLICY_VERSION {
        migration.push_str(bundle);
    } else if retained_prefix {
        if review && !manifest["inject_review"].as_bool().unwrap_or(false) {
            migration.push_str(crate::REVIEW_POLICY);
        }
        if implement && !manifest["inject_implement"].as_bool().unwrap_or(false) {
            migration.push_str(crate::IMPLEMENTATION_POLICY);
        }
        if figure && !manifest["inject_fig"].as_bool().unwrap_or(false) {
            migration.push_str(crate::FIGURE_POLICY);
        }
        if write && !manifest["inject_write"].as_bool().unwrap_or(false) {
            migration.push_str(crate::WRITING_APPLICATION);
            migration.push_str("\n\n");
            migration.push_str(crate::WRITING_POLICY);
        }
    }
    if options.allow_subteam
        && retained_prefix
        && manifest["worker_policy_version"] == crate::WORKER_POLICY_VERSION
        && manifest["coordinator_policy_version"] != crate::COORDINATOR_POLICY_VERSION
    {
        migration.push_str("\n\n");
        migration.push_str(crate::subteam::POLICY);
    }
    migration
}

fn generate_stage(
    options: &Options,
    run: &Path,
    dir: &Path,
    manifest: &mut Value,
    baseline: &Value,
    review_stage: bool,
) -> Result<String, String> {
    let _home = IsolatedHome::new(run, &options.authentication)?;
    let artifacts = dir.join("artifacts");
    private_dir(&artifacts).map_err(|e| e.to_string())?;
    let final_handoff = artifacts.join(options.contract.handoff_name());
    let checkpoint = artifacts.join("review-checkpoint.md");
    let handoff = if review_stage {
        &checkpoint
    } else {
        &final_handoff
    };
    for store in [&options.ctx_store, &options.dec_store] {
        fs::create_dir_all(store).map_err(|e| format!("create tool store: {e}"))?;
    }
    let scratch = run.join("scratch");
    if !scratch.exists() {
        private_dir(&scratch).map_err(|e| e.to_string())?;
    }
    private_path(&scratch, true)?;
    let (bundle, guidance) = policy_bundle(options, manifest, review_stage);
    let mut task = if manifest["stage"] == "implementation" && !review_stage {
        format!(
            include_str!("implementation_task.md"),
            manifest["task"].as_str().unwrap_or(&options.task),
            manifest["review_checkpoint"]
        )
    } else {
        options.task.clone()
    };
    for index in 0..=1 {
        let attempt_dir = if index == 0 {
            dir.to_owned()
        } else {
            dir.join("repair")
        };
        if index > 0 {
            private_dir(&attempt_dir).map_err(|e| e.to_string())?;
        }
        if crate::CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("interrupted before worker launch".into());
        }
        // Preserve the prefix for retained conversations; migration is injected once below.
        let initial_policy = run.join("policy.md");
        let policy = if initial_policy.exists() {
            fs::read_to_string(&initial_policy).map_err(|e| e.to_string())?
        } else {
            bundle.clone()
        };
        let migration = policy_additions(
            options,
            manifest,
            &bundle,
            guidance,
            initial_policy.exists(),
        );
        let phase = format!(
            include_str!("assignment_contract.md"),
            migration,
            json!({"workspace":options.cwd,"handoff_path":handoff,"scratch":scratch,
                "completion_gate":manifest["completion_gate"],"output_contract":options.contract.description(),
                "stage":manifest["stage"],"review_checkpoint":manifest["review_checkpoint"],
                "ctx_store":options.ctx_store,"dec_store":options.dec_store,
                "execution_permissions":manifest["execution_permissions"]}),
            if index > 0 {
                include_str!("repair_stage.md")
            } else if review_stage {
                include_str!("review_stage.md")
            } else if manifest["stage"] == "implementation" {
                include_str!("implementation_stage.md")
            } else {
                include_str!("edit_authority.md")
            }
        );
        create(&attempt_dir.join("policy.md"))?
            .write_all(policy.as_bytes())
            .map_err(|e| e.to_string())?;
        create(&attempt_dir.join("phase.md"))?
            .write_all(phase.as_bytes())
            .map_err(|e| e.to_string())?;
        create(&attempt_dir.join("task.txt"))?
            .write_all(task.as_bytes())
            .map_err(|e| e.to_string())?;
        manifest["repairs"] = json!(index);
        let attempt = manifest["attempts"].as_array().unwrap().len();
        let record = json!({"logs":attempt_dir,"status":"running","stage":manifest["stage"]});
        manifest["attempts"].as_array_mut().unwrap().push(record);
        save_json(&run.join("manifest.json"), manifest)?;
        let turn = match app_server::turn(
            options,
            run,
            &attempt_dir,
            &task,
            manifest,
            handoff,
            guidance,
        ) {
            Ok(turn) => turn,
            Err(error) => {
                manifest["attempts"][attempt]["status"] = json!(if crate::CANCELLED
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    "interrupted"
                } else {
                    "failed"
                });
                manifest["attempts"][attempt]["error"] = json!(error);
                manifest["usage_complete"] = json!(false);
                return Err(error);
            }
        };
        manifest["attempts"][attempt]["usage"] = json!(turn.usage);
        manifest["attempts"][attempt]["status"] = json!(turn.status);
        usage_update(manifest, turn.usage, baseline);
        let candidate = turn.text.unwrap_or_default();
        let candidate_path = attempt_dir.join("candidate.txt");
        create(&candidate_path)?
            .write_all(candidate.as_bytes())
            .map_err(|e| e.to_string())?;
        manifest["attempts"][attempt]["candidate"] = json!(candidate_path);
        if turn.status != "completed" || turn.error.is_some() {
            manifest["status"] = json!(if turn.status == "completed" {
                "failed"
            } else {
                &turn.status
            });
            manifest["usage_complete"] = json!(false);
            let error = turn
                .error
                .unwrap_or_else(|| format!("worker {}", turn.status));
            manifest["attempts"][attempt]["status"] = manifest["status"].clone();
            manifest["attempts"][attempt]["error"] = json!(error);
            return Err(error);
        }
        // A checkpoint has its own minimal contract, never the final schema/columns.
        // Invalid checkpoints fail closed without spending the final format repair.
        if review_stage && index == 0 && artifact::Contract::control(&candidate)?.0 == "completed" {
            let checkpoint_contract = artifact::Contract::new("markdown".into(), None, None)?;
            let result = checkpoint_contract.validate_file(&candidate, handoff)?;
            manifest["review_checkpoint"] = json!(checkpoint);
            manifest["attempts"][attempt]["status"] = json!("checkpoint_validated");
            return Ok(result.content);
        }
        match options.contract.validate_file(&candidate, handoff) {
            Ok(result) => {
                if review_stage && result.status == "completed" {
                    return Err(
                        "review blocker repair must preserve the non-completed outcome".into(),
                    );
                }
                let path = &final_handoff;
                if review_stage {
                    // Publish only the blocker report, keeping the checkpoint internal.
                    create(path)?
                        .write_all(result.content.as_bytes())
                        .map_err(|e| e.to_string())?;
                }
                manifest["attempts"][attempt]["status"] = json!("validated");
                manifest["result"] = json!(path);
                manifest["artifact"] = json!(
                    path.strip_prefix(run)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                );
                manifest["format"] = json!(result.format);
                manifest["status"] = json!(result.status);
                return Ok(result.content);
            }
            Err(error) => {
                if let Ok(content) = artifact::read_handoff_bytes(handoff) {
                    create(&attempt_dir.join("rejected-handoff.txt"))?
                        .write_all(&content)
                        .map_err(|e| e.to_string())?;
                }
                let diagnostics = attempt_dir.join("validation.json");
                save_json(&diagnostics, &json!({"error":error}))?;
                manifest["attempts"][attempt]["status"] = json!("invalid");
                manifest["attempts"][attempt]["diagnostics"] = json!(diagnostics);
                save_json(&run.join("manifest.json"), manifest)?;
                if index == 1 {
                    return Err("artifact validation failed after one repair; inspect candidate and validation.json".into());
                }
                task = format!(
                    include_str!("repair_task.md"),
                    json!(candidate_path),
                    json!(diagnostics),
                    json!(handoff)
                );
            }
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_bundle_composes_each_selection_without_changing_default_scope() {
        for mask in 0..16 {
            let mut options = crate::tests::options(&["task"]).unwrap();
            options.inject_review = mask & 1 != 0;
            options.inject_implement = mask & 2 != 0;
            options.inject_fig = mask & 4 != 0;
            options.inject_write = mask & 8 != 0;
            let manifest = json!({"worker_policy_version":crate::WORKER_POLICY_VERSION});
            let (bundle, selected) = policy_bundle(&options, &manifest, false);
            assert_eq!(
                selected,
                (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0)
            );
            let mut expected = format!(
                "{}\n\n{}{}",
                crate::POLICY,
                artifact::INSTRUCTIONS,
                include_str!("ordinary_worker.md")
            );
            for (enabled, text) in [
                (mask & 1 != 0, crate::REVIEW_POLICY),
                (mask & 2 != 0, crate::IMPLEMENTATION_POLICY),
                (mask & 4 != 0, crate::FIGURE_POLICY),
                (mask & 8 != 0, crate::WRITING_APPLICATION),
                (mask & 8 != 0, crate::WRITING_POLICY),
            ] {
                if enabled {
                    expected.push_str("\n\n");
                    expected.push_str(text);
                }
            }
            assert_eq!(bundle, expected, "mask={mask}");
            assert!(policy_additions(&options, &manifest, &bundle, selected, false).is_empty());
        }
    }

    #[test]
    fn retained_writing_addition_is_acknowledged_once_including_repair_or_failed_turn_resume() {
        let mut options = crate::tests::options(&["task", "--inject-write"]).unwrap();
        let mut manifest = json!({"worker_policy_version":crate::WORKER_POLICY_VERSION});
        let (bundle, selected) = policy_bundle(&options, &manifest, false);
        assert_eq!(selected, (false, false, false, true));
        assert_eq!(
            policy_additions(&options, &manifest, &bundle, selected, true),
            format!(
                "{}\n\n{}",
                crate::WRITING_APPLICATION,
                crate::WRITING_POLICY
            )
        );
        // app_server records acknowledgement before turn/start; repair and failed
        // turns therefore use this state, not the eventual worker outcome.
        manifest["inject_write"] = json!(true);
        for requested_again in [true, false] {
            options.inject_write = requested_again;
            let (bundle, selected) = policy_bundle(&options, &manifest, false);
            assert_eq!(selected, (false, false, false, true));
            assert!(policy_additions(&options, &manifest, &bundle, selected, true).is_empty());
        }
        let args: Vec<String> = ["resume", "id", "--inject-write", "--inject-write"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            existing(&args).unwrap_err(),
            "provide --inject-write only once"
        );
    }

    #[test]
    fn writing_survives_staged_review_without_loading_implementation_early() {
        let options = crate::tests::options(&[
            "task",
            "--inject-review",
            "--inject-implement",
            "--inject-write",
        ])
        .unwrap();
        let mut manifest =
            json!({"worker_policy_version":crate::WORKER_POLICY_VERSION,"stage":"review"});
        let (review, selected) = policy_bundle(&options, &manifest, true);
        assert_eq!(selected, (true, false, false, true));
        assert!(review.contains(crate::REVIEW_POLICY));
        assert!(review.contains(crate::WRITING_POLICY));
        assert!(!review.contains(crate::IMPLEMENTATION_POLICY));
        manifest["inject_review"] = json!(true);
        manifest["inject_write"] = json!(true);
        manifest["stage"] = json!("implementation");
        // Stage state alone resumes implementation after interruption.
        let resumed = crate::tests::options(&["task"]).unwrap();
        let (implementation, selected) = policy_bundle(&resumed, &manifest, false);
        assert_eq!(selected, (true, true, false, true));
        assert_eq!(
            policy_additions(&resumed, &manifest, &implementation, selected, true),
            crate::IMPLEMENTATION_POLICY
        );
        manifest["inject_implement"] = json!(true);
        assert!(policy_additions(&resumed, &manifest, &implementation, selected, true).is_empty());
    }

    #[test]
    fn migration_includes_writing_and_preserves_legacy_review_and_implementation() {
        let options = crate::tests::options(&["task", "--inject-write"]).unwrap();
        for version in [5, 19] {
            let manifest = json!({"worker_policy_version":version});
            let (bundle, selected) = policy_bundle(&options, &manifest, false);
            assert_eq!(selected, (version < 6, version < 6, false, true));
            assert_eq!(
                policy_additions(&options, &manifest, &bundle, selected, true),
                bundle
            );
            assert_eq!(bundle.matches(crate::WRITING_POLICY).count(), 1);
        }
    }

    #[test]
    fn coordinator_refresh_is_independent_of_common_policy_and_idempotent() {
        let mut options = crate::tests::options(&["task", "--allow-subteam"]).unwrap();
        let mut manifest = json!({"worker_policy_version":crate::WORKER_POLICY_VERSION,"coordinator_policy_version":20});
        let (bundle, selected) = policy_bundle(&options, &manifest, false);
        let coordinator = format!("\n\n{}", crate::subteam::POLICY);
        assert_eq!(
            policy_additions(&options, &manifest, &bundle, selected, true),
            coordinator
        );
        // Simultaneous writing selection adds only writing and the coordinator refresh.
        options.inject_write = true;
        let (bundle, selected) = policy_bundle(&options, &manifest, false);
        assert_eq!(
            policy_additions(&options, &manifest, &bundle, selected, true),
            format!(
                "{}\n\n{}{}",
                crate::WRITING_APPLICATION,
                crate::WRITING_POLICY,
                coordinator
            )
        );
        manifest["inject_write"] = json!(true);
        manifest["coordinator_policy_version"] = json!(crate::COORDINATOR_POLICY_VERSION);
        assert!(policy_additions(&options, &manifest, &bundle, selected, true).is_empty());
        // An older common policy migrates the complete bundle once, not a second coordinator copy.
        manifest["worker_policy_version"] = json!(19);
        assert_eq!(
            policy_additions(&options, &manifest, &bundle, selected, true),
            bundle
        );
        assert_eq!(bundle.matches(crate::subteam::POLICY).count(), 1);
    }

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pira-team-state-{}-{stamp}-{sequence}",
                std::process::id()
            ));
            private_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn literal_parent_path(path: &Path) -> PathBuf {
        // PathBuf::join normalizes parent components for Windows verbatim paths.
        let mut raw = path.as_os_str().to_os_string();
        raw.push(std::path::MAIN_SEPARATOR_STR);
        raw.push("..");
        let path = PathBuf::from(raw);
        assert!(
            path.components()
                .any(|component| component == Component::ParentDir),
            "parent fixture was normalized: {path:?}"
        );
        path
    }

    #[test]
    fn decision_anchor_capture_and_retention_preserve_canonical_scope() {
        let dir = TestDir::new();
        let root = dir.0.canonicalize().unwrap();
        let child = root.join("child");
        private_dir(&child).unwrap();
        assert_eq!(decision_workspace(&child).unwrap(), child);
        assert_eq!(
            retained_dec_workspace(&json!({"dec_workspace":child})).unwrap(),
            Some(child.clone())
        );
        private_dir(&root.join(".git")).unwrap();
        assert_eq!(decision_workspace(&child).unwrap(), root);
        assert_eq!(
            retained_dec_workspace(&json!({"dec_workspace":root})).unwrap(),
            Some(root)
        );
        assert_eq!(retained_dec_workspace(&json!({})).unwrap(), None);
        assert_eq!(
            retained_dec_workspace(&json!({"dec_workspace":null})).unwrap(),
            None
        );
        for value in [json!("relative"), json!([]), json!(dir.0.join("missing"))] {
            assert!(retained_dec_workspace(&json!({"dec_workspace":value})).is_err());
        }
        let parent = literal_parent_path(&child);
        assert!(retained_dec_workspace(&json!({"dec_workspace":parent})).is_err());
    }

    #[test]
    fn retained_dec_workspace_rejects_parent_components_before_io() {
        let dir = TestDir::new();
        let path = literal_parent_path(&dir.0.canonicalize().unwrap().join("missing"));
        assert_eq!(
            retained_dec_workspace(&json!({"dec_workspace":path})).unwrap_err(),
            "persisted dec_workspace must remain the same canonical directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn retained_dec_workspace_rejects_symlink_rebinding() {
        let dir = TestDir::new();
        let root = dir.0.canonicalize().unwrap();
        let original = root.join("original");
        let other = root.join("other");
        private_dir(&original).unwrap();
        private_dir(&other).unwrap();
        let manifest = json!({"dec_workspace":decision_workspace(&original).unwrap()});
        fs::remove_dir(&original).unwrap();
        std::os::unix::fs::symlink(&other, &original).unwrap();
        assert!(retained_dec_workspace(&manifest).is_err());
    }

    #[test]
    fn atomic_save_never_removes_a_preexisting_temporary_file() {
        let dir = TestDir::new();
        let path = dir.0.join("manifest.json");
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&path, b"original manifest").unwrap();
        fs::write(&temporary, b"not owned by this save").unwrap();
        assert!(save_json(&path, &json!({"status":"completed"})).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original manifest");
        assert_eq!(fs::read(&temporary).unwrap(), b"not owned by this save");
    }

    #[test]
    fn atomic_save_replaces_metadata_and_cleans_only_its_own_failed_write() {
        let dir = TestDir::new();
        let path = dir.0.join("manifest.json");
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        save_json(&path, &json!({"status":"running"})).unwrap();
        let complete = json!({"status":"completed","note":"Unicode λ"});
        save_json(&path, &complete).unwrap();
        assert_eq!(read_json(&path).unwrap(), complete);
        assert!(!temporary.exists());

        let directory = dir.0.join("directory.json");
        private_dir(&directory).unwrap();
        assert!(save_json(&directory, &complete).is_err());
        assert!(directory.is_dir());
        assert!(
            !directory
                .with_extension(format!("{}.tmp", std::process::id()))
                .exists()
        );
    }

    #[test]
    fn initial_revision_publication_failure_releases_empty_allocation_for_retry() {
        let dir = TestDir::new();
        let path = dir.0.join("manifest.json");
        let snapshot = dir.0.join("000002");
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        save_json(&path, &json!({"revision":1,"status":"completed"})).unwrap();
        let original = fs::read(&path).unwrap();
        fs::write(&temporary, b"unrelated temporary file").unwrap();
        let next = json!({"revision":2,"status":"running"});
        assert!(begin_revision(&snapshot, || save_json(&path, &next)).is_err());
        assert!(!snapshot.exists());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&temporary).unwrap(), b"unrelated temporary file");
        fs::remove_file(&temporary).unwrap();
        begin_revision(&snapshot, || save_json(&path, &next)).unwrap();
        assert!(snapshot.is_dir());
        assert_eq!(read_json(&path).unwrap(), next);
    }

    #[test]
    fn initial_revision_publication_failure_preserves_newly_arrived_payload() {
        let dir = TestDir::new();
        let snapshot = dir.0.join("000002");
        let payload = snapshot.join("keep");
        let error = begin_revision(&snapshot, || {
            fs::write(&payload, b"concurrent payload").unwrap();
            Err("publication failed".into())
        })
        .unwrap_err();
        assert!(error.starts_with("publication failed; uncommitted revision retained at "));
        assert_eq!(fs::read(&payload).unwrap(), b"concurrent payload");
    }

    #[test]
    fn revision_allocation_never_removes_preexisting_state_or_publishes_over_it() {
        let dir = TestDir::new();
        let snapshot = dir.0.join("000002");
        private_dir(&snapshot).unwrap();
        for populated in [false, true] {
            if populated {
                fs::write(snapshot.join("keep"), b"original payload").unwrap();
            }
            assert!(begin_revision(&snapshot, || panic!("must not publish")).is_err());
            assert!(snapshot.is_dir());
            if populated {
                assert_eq!(
                    fs::read(snapshot.join("keep")).unwrap(),
                    b"original payload"
                );
            }
        }
    }

    #[test]
    fn usage_preserves_totals_and_latches_incomplete_reporting() {
        let baseline = json!({"input_tokens":10,"cached_input_tokens":2,"output_tokens":4});
        let mut manifest = json!({"usage":baseline,"usage_complete":true,"revision_usage":{}});
        let report = json!({"input_tokens":17,"cached_input_tokens":3,"output_tokens":7});
        usage_update(&mut manifest, Some(report.clone()), &baseline);
        assert_eq!(
            manifest["revision_usage"],
            json!({"input_tokens":7,"cached_input_tokens":1,"output_tokens":3})
        );
        // Repair reports cumulative usage, not another independent amount to sum.
        let repaired = json!({"input_tokens":25,"cached_input_tokens":4,"output_tokens":9});
        usage_update(&mut manifest, Some(repaired.clone()), &baseline);
        assert_eq!(manifest["usage"], repaired);
        assert_eq!(manifest["revision_usage"]["input_tokens"], 15);
        for invalid in [
            None,
            Some(Value::Null),
            Some(json!([])),
            Some(json!({"input_tokens":30})),
            Some(report),
            Some(json!({"input_tokens":30,"cached_input_tokens":5,"output_tokens":10,"extra":-1})),
        ] {
            usage_update(&mut manifest, invalid, &baseline);
            assert_eq!(manifest["usage"], repaired);
            assert_eq!(manifest["revision_usage"]["input_tokens"], 15);
            assert_eq!(manifest["usage_complete"], false);
        }
        usage_update(&mut manifest, Some(repaired), &baseline);
        assert_eq!(manifest["usage_complete"], false);
    }
}
