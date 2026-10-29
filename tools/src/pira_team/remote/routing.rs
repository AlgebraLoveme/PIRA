//! Local routing metadata is not a native run manifest or a delegation grant.
//! Each uncertain effect retains its signed identity before any SSH submission.
use super::*;
use crate::bookmarks;

const ROUTES: &str = ".remote-routes";

struct Route {
    dir: PathBuf,
    id: String,
    authority: PathBuf,
    envelope: Value,
}

fn present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

impl Route {
    fn load(root: &Path, id: &str, issuer: &str) -> Result<Option<Self>, String> {
        storage::validate_run_id(id)?;
        let Some(root) = profile::defaults::root_if_present(root)? else {
            return Ok(None);
        };
        let routes = root.join(ROUTES);
        if !present(&routes)? {
            return Ok(None);
        }
        existing(&routes, true)?;
        let dir = routes.join(id);
        if !present(&dir)? {
            return Ok(None);
        }
        existing(&dir, true)?;
        if present(&root.join(id))? {
            return Err("ambiguous local/remote run ID; no operation submitted".into());
        }
        let record = read(&dir.join("route.json"))?;
        if record.as_object().is_none_or(|o| o.len() != 4)
            || record["version"] != 1
            || record["run_id"] != id
            || record["envelope"]["grant"]["issuer"] != issuer
        {
            return Err("remote route unavailable/malformed for this main session".into());
        }
        let authority = PathBuf::from(text(&record, "authority")?);
        if !authority.is_absolute() {
            return Err("invalid local route authority path".into());
        }
        let route = Self {
            dir,
            id: id.into(),
            authority,
            envelope: record["envelope"].clone(),
        };
        let grant = route.verify(&route.envelope, issuer)?;
        if grant["op"] != "submit"
            || grant["args"][0] != "run"
            || format!("remote-{}", text(&grant, "nonce")?) != id
        {
            return Err("remote route differs from its signed initial allocation".into());
        }
        Ok(Some(route))
    }

    fn verify(&self, envelope: &Value, issuer: &str) -> Result<Value, String> {
        // Only public owner metadata is read here; signing secrets remain in ssh-keygen.
        existing(&self.authority, false)?;
        let owner_path = appended(&self.authority, ".owner.json");
        existing(&owner_path, false)?;
        let owner = read(&owner_path)?;
        let public = text(&owner, "public")?;
        if owner["version"] != VERSION
            || owner["issuer"] != issuer
            || envelope["grant"]["issuer"] != issuer
            || public.split_whitespace().count() != 2
            || !public.starts_with("ssh-ed25519 ")
            || public.contains(['\n', '\r'])
        {
            return Err("remote route authority is not bound to this verified main".into());
        }
        let anchor = format!("{issuer} namespaces=\"{NAMESPACE}\" {public}\n");
        verify_with_keys(envelope, anchor.as_bytes())
    }

    fn cli(&self, grant: &Value) -> Result<Client, String> {
        Ok(Client {
            endpoint: self.envelope["grant"]["endpoint"].clone(),
            authority: self.authority.clone(),
            state: self.dir.clone(),
            id: text(grant, "request_id")?.into(),
            run_request: grant["run_request"].as_str().map(str::to_owned),
            operation: grant["operation_id"].as_str().map(str::to_owned),
            cwd: grant["cwd"].as_str().map(str::to_owned),
            detach: false,
            artifact: false,
            file: None,
            args: strings(&grant["args"])?,
        })
    }

    fn pending(&self, op: &str, issuer: &str) -> Result<Option<Value>, String> {
        let path = self.dir.join(format!("pending-{op}.json"));
        if !present(&path)? {
            return Ok(None);
        }
        let envelope = read(&path)?;
        let grant = self.verify(&envelope, issuer)?;
        same_owner(&self.envelope["grant"], &grant)?;
        let expected = if op == "resume" { "submit" } else { op };
        if grant["op"] != expected
            || grant["args"][0] != op
            || (op == "resume" && grant["run_request"] != self.envelope["grant"]["request_id"])
            || (op != "resume" && grant["request_id"] != self.envelope["grant"]["request_id"])
        {
            return Err("pending operation differs from its signed route binding".into());
        }
        Ok(Some(envelope))
    }

    fn done_path(&self, grant: &Value) -> Result<PathBuf, String> {
        let id = if grant["op"] == "submit" {
            text(grant, "request_id")?
        } else {
            text(grant, "operation_id")?
        };
        key(id)?;
        Ok(self.dir.join(format!("delivered-{id}.json")))
    }

    fn delivered(&self, grant: &Value) -> Result<bool, String> {
        let path = self.done_path(grant)?;
        if !present(&path)? {
            return Ok(false);
        }
        if read(&path)? != json!({"intent":logical(grant)}) {
            return Err("invalid retained operation delivery marker".into());
        }
        Ok(true)
    }

    fn mark_delivered(&self, grant: &Value) -> Result<(), String> {
        if !self.delivered(grant)? {
            write_new(&self.done_path(grant)?, &json!({"intent":logical(grant)}))?;
        }
        Ok(())
    }

    fn superseded(&self, envelope: &Value) -> Result<bool, String> {
        let path = self
            .done_path(&envelope["grant"])?
            .with_extension("superseded.json");
        if !present(&path)? {
            return Ok(false);
        }
        let record = read(&path)?;
        validate_target(&record["observed_target"])?;
        if record["envelope"] != *envelope
            || record["outcome"] != "unknown"
            || record["observed_target"] == envelope["grant"]["target"]
        {
            return Err("invalid superseded-control record".into());
        }
        Ok(true)
    }

    fn observe_controls(&self, issuer: &str, target: &Value) -> Result<(), String> {
        validate_target(target)?;
        for op in ["steer", "interrupt"] {
            if let Some(envelope) = self.pending(op, issuer)?
                && !self.delivered(&envelope["grant"])?
                && !self.superseded(&envelope)?
                && envelope["grant"]["target"] != *target
            {
                // Retain unknown outcome and old signed identity. Observing a
                // different native epoch does not acknowledge/reissue its effect.
                let path = self
                    .done_path(&envelope["grant"])?
                    .with_extension("superseded.json");
                write_new(
                    &path,
                    &json!({"envelope":envelope,"observed_target":target,"outcome":"unknown"}),
                )?;
            }
        }
        Ok(())
    }
}

pub(crate) fn reference_for(root: &Path, id: &str, issuer: &str) -> Result<Option<Value>, String> {
    Ok(Route::load(root, id, issuer)?.map(|_| json!({"run_id":id,"run_root":null})))
}

// Compatibility submissions with explicit request IDs use the same ordinary
// route index, irrespective of their explicit transport controller directory.
pub(super) fn retain_transport(cli: &Client, envelope: &Value) -> Result<(), String> {
    let root = profile::defaults::create_root(&storage::default_root()?)?;
    let route = retain_at(&root, cli, envelope)?;
    eprintln!("pira_team run_id: {}", route.id);
    Ok(())
}

fn retain_at(root: &Path, cli: &Client, envelope: &Value) -> Result<Route, String> {
    validate_grant(&envelope["grant"])?;
    let routes = root.join(ROUTES);
    directory(&routes)?;
    let issuer = text(&envelope["grant"], "issuer")?;
    let id = format!("remote-{}", text(&envelope["grant"], "nonce")?);
    if let Some(route) = Route::load(&root, &id, issuer)? {
        if logical(&route.envelope["grant"]) != logical(&envelope["grant"])
            || route.authority != cli.authority
        {
            return Err("retained route differs from transport submission".into());
        }
        return Ok(route);
    } else {
        if present(&root.join(&id))? {
            return Err("ambiguous local/remote run ID".into());
        }
        let dir = routes.join(&id);
        directory(&dir)?;
        write_new(
            &dir.join("route.json"),
            &json!({"version":1,"run_id":id,
            "authority":cli.authority,"envelope":envelope}),
        )?;
    }
    Route::load(root, &id, issuer)?.ok_or("retained route disappeared".into())
}

pub(super) fn resolve(args: &[String]) -> Result<(), String> {
    let context = verified_root(args)?;
    let (root, target) = selection(args)?;
    if args.iter().enumerate().skip(1).any(|(i, arg)| {
        i != target
            && arg != "--store"
            && args.get(i.wrapping_sub(1)).map(String::as_str) != Some("--store")
    }) {
        return Err("expected resolve REQUEST_ID [--store DIR]".into());
    }
    key(&args[target])?;
    let issuer = text(&context, "caller_thread_id")?;
    let root = profile::defaults::root_if_present(&root)?
        .ok_or("request unavailable to this main session")?;
    let routes = root.join(ROUTES);
    existing(&routes, true)?;
    let mut found = None;
    for entry in fs::read_dir(&routes).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        existing(&entry.path(), true)?;
        let id = entry
            .file_name()
            .into_string()
            .map_err(|_| "invalid route name")?;
        // A route for another main is not this caller's authority to inspect.
        let record = read(&entry.path().join("route.json"))?;
        if record["envelope"]["grant"]["issuer"] != issuer {
            continue;
        }
        let route = Route::load(&root, &id, issuer)?.ok_or("route disappeared")?;
        if route.envelope["grant"]["request_id"] == args[target] {
            if found.is_some() {
                return Err(
                    "request ID is ambiguous across retained routes; use the pre-submit run_id"
                        .into(),
                );
            }
            found = Some(route.id);
        }
    }
    let id = found.ok_or("request unavailable to this main session")?;
    println!(
        "{}",
        json!({"run_id":id,"status":"unknown","run_root":null,"handoff_path":null})
    );
    Ok(())
}

fn execution(context: &Value) -> Value {
    json!({"caller_thread_id":context["caller_thread_id"],"model":context["model"],"effort":context["effort"],
        "approval_policy":context["approval_policy"],"sandbox_policy":context["sandbox_policy"],
        "permission_profile":context["permission_profile"],"file_system_sandbox_policy":context["file_system_sandbox_policy"]})
}

fn set_launch_identity(args: &mut Vec<String>, root: &Path, request: String) {
    let mut generated = vec!["--request-id".into(), request];
    if !has_option(args, "--remote-state") {
        generated.extend([
            "--remote-state".into(),
            root.join(".remote-controller")
                .to_string_lossy()
                .into_owned(),
        ]);
    }
    args.splice(1..1, generated);
}

pub(super) fn launch(args: &[String]) -> Result<(), String> {
    let context = verified_root(args)?;
    let issuer = text(&context, "caller_thread_id")?;
    execute_policy(&context, issuer)?;
    let mut forwarded = vec!["run".into()];
    let (mut store, mut label) = (None, None);
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        let name = arg.split('=').next().unwrap();
        if matches!(name, "--store" | "--label") {
            let value = match arg.split_once('=') {
                Some((_, value)) => value.to_owned(),
                None => rest
                    .next()
                    .ok_or_else(|| format!("missing {name}"))?
                    .clone(),
            };
            crate::set_once(
                if name == "--store" {
                    &mut store
                } else {
                    &mut label
                },
                value,
                name,
            )?;
        } else {
            forwarded.push(arg.clone());
            if arg == "--" {
                forwarded.extend(rest.cloned());
                break;
            }
            if arg.starts_with('-') && !arg.contains('=') && !flag(arg) {
                forwarded.push(rest.next().ok_or_else(|| format!("missing {arg}"))?.clone());
            }
        }
    }
    if let Some(label) = &label {
        bookmarks::validate_label(label)?;
    }
    if store.as_deref() == Some("") {
        return Err("--store must not be empty".into());
    }
    let root = store
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(storage::default_root)?;
    let root = profile::defaults::create_root(&root)?;
    let routes = root.join(ROUTES);
    directory(&routes)?;
    let request = random()?;
    // Legacy explicit request-ID transport remains available. Normal launch IDs
    // and their location are generated and retained by the launcher, not MAIN.
    set_launch_identity(&mut forwarded, &root, request);
    let cli = Client::parse(&forwarded)?;
    let grant = cli.grant("submit", issuer, Some(&execution(&context)))?;
    validate_grant(&grant)?;
    let envelope = sign(&grant, &cli.authority)?;
    directory(&cli.state)?;
    write_new(&cli.state.join(format!("{}.json", cli.id)), &envelope)?;
    let route = retain_at(&root, &cli, &envelope)?;
    // Publish the recoverable ID before even capability negotiation. A lost
    // connection or killed client never requires a second launch.
    eprintln!("pira_team run_id: {}", route.id);
    eprintln!(
        "pira_team: remote run retained; use read {} manifest.json after disconnection; only explicit resume starts another turn",
        route.id
    );
    if let Some(label) = label {
        bookmarks::assign(&root, &route.id, &label)?;
    }
    cli.negotiate(issuer, "run", requests_subteam(&cli.args))?;
    let result = cli
        .exchange(&route.envelope)
        .map_err(|e| recovery_error(&route, e))?;
    validate_view(&route, &result, &cli.id)?;
    let result = if cli.detach {
        result
    } else {
        wait(&route, &cli, issuer, result)?
    };
    output(
        &route,
        &cli,
        issuer,
        &result,
        output_mode(&cli.args, "artifact")?,
    )
}

fn recovery_error(route: &Route, error: String) -> String {
    format!(
        "{error}; retained run {}; use ordinary read {} manifest.json to recover, not another run/resume",
        route.id, route.id
    )
}

// Parse only routing selection, leaving full option validation to the operation.
fn selection(args: &[String]) -> Result<(PathBuf, usize), String> {
    let mut root = None;
    let mut target = None;
    let mut index = 1;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            target.get_or_insert(index + 1);
            break;
        }
        if arg == "--store" {
            index += 1;
            root = Some(PathBuf::from(args.get(index).ok_or("missing --store")?));
        } else if !arg.starts_with('-') {
            target.get_or_insert(index);
        } else if !arg.contains('=') && !flag(arg) {
            index += 1;
        }
        index += 1;
    }
    let target = target.filter(|i| *i < args.len()).ok_or("missing RUN_ID")?;
    Ok((root.map(Ok).unwrap_or_else(storage::default_root)?, target))
}

pub(super) fn dispatch(args: &mut [String]) -> Option<Result<(), String>> {
    dispatch_using(args, crate::subteam::authorize)
}

fn dispatch_using(
    args: &mut [String],
    authorize: impl FnOnce(&[String]) -> Result<(), String>,
) -> Option<Result<(), String>> {
    if !matches!(
        args[0].as_str(),
        "read" | "path" | "resume" | "steer" | "interrupt" | "tier" | "wait"
    ) {
        return None;
    }
    if let Err(error) = authorize(args) {
        return Some(Err(error));
    }
    let (root, target) = match selection(args) {
        Ok(value) => value,
        Err(e) => return Some(Err(e)),
    };
    let issuer = match profile::caller_session() {
        Ok(value) => value,
        Err(e) => return Some(Err(e)),
    };
    if let Some(label) = args[target].strip_prefix('@') {
        args[target] = match bookmarks::resolve_for(&root, label, &issuer) {
            Ok(id) => id,
            Err(e) => return Some(Err(e)),
        };
    }
    let route = match Route::load(&root, &args[target], &issuer) {
        Ok(Some(route)) => route,
        Ok(None) => return None,
        Err(e) => return Some(Err(e)),
    };
    Some(ordinary(&route, args, target))
}

fn validate_view(route: &Route, view: &Value, request: &str) -> Result<(), String> {
    if view["accepted"] != true || view["run_id"] != route.id || view["request_id"] != request {
        return Err("remote response differs from retained run/request identity".into());
    }
    let manifest = &view["manifest"];
    if !manifest.is_null()
        && (manifest["run_id"] != route.id
            || manifest["creator_main_session_id"] != route.envelope["grant"]["issuer"]
            || manifest.get("parent_run_id") != Some(&Value::Null))
    {
        return Err("remote manifest differs from signed root ownership".into());
    }
    Ok(())
}

fn observe(
    route: &Route,
    cli: &Client,
    context: &Value,
    envelope: &Value,
) -> Result<Value, String> {
    let issuer = text(context, "caller_thread_id")?;
    let mut result = cli.observe(issuer).map_err(|e| recovery_error(route, e))?;
    if result["accepted"] == false
        && result["status"] == "not_found"
        && result["request_id"] == cli.id
    {
        // No server claim exists. Recover the exact intended turn, never resume.
        // Refresh validity only with current equivalent execution authority and
        // keep the original nonce/allocation, request ID and signed payload.
        execute_policy(context, issuer)?;
        if execution(context) != envelope["grant"]["context"] {
            return Err("unaccepted submission requires its original execution context; no replay under changed authority".into());
        }
        let mut grant = envelope["grant"].clone();
        let time = now()?;
        grant["issued_at"] = json!(time);
        grant["expires_at"] = json!(time + VALIDITY);
        let refreshed = sign(&grant, &route.authority)?;
        let metadata = cli.metadata(issuer)?;
        if cli.args[0] == "resume" {
            let initial = route.cli(&route.envelope["grant"])?;
            let retained = initial.observe(issuer)?;
            require_resume_capabilities(&metadata, &retained, &route.envelope["grant"], &cli.args)?;
        } else {
            require_capabilities(&metadata, "run", requests_subteam(&cli.args))?;
        }
        result = cli
            .exchange(&refreshed)
            .map_err(|e| recovery_error(route, e))?;
    }
    validate_view(route, &result, &cli.id)?;
    Ok(result)
}

fn wait(route: &Route, cli: &Client, issuer: &str, mut result: Value) -> Result<Value, String> {
    let startup = std::time::Instant::now();
    loop {
        validate_view(route, &result, &cli.id)?;
        if !result["operation_result"].is_null() && result["manifest"]["status"] != "running" {
            return Ok(result);
        }
        if crate::CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(recovery_error(
                route,
                "waiting client disconnected; server worker was not interrupted".into(),
            ));
        }
        // A freshly accepted supervisor can be starting before its lock appears.
        // Do not mistake that race for completion or launch a replacement.
        if result["accounting_may_be_incomplete"] == true
            && (!result["manifest"].is_null() || startup.elapsed() > Duration::from_secs(5))
        {
            return Err(recovery_error(
                route,
                "supervisor settlement unknown; inspect retained diagnostics; no automatic replay"
                    .into(),
            ));
        }
        std::thread::sleep(Duration::from_secs(1));
        result = cli.observe(issuer).map_err(|e| recovery_error(route, e))?;
    }
}

fn output_mode(args: &[String], default: &str) -> Result<String, String> {
    let mut result = default.to_owned();
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break;
        }
        if arg == "--output" {
            result = rest.next().ok_or("missing --output")?.clone();
        } else if arg.starts_with('-') && !arg.contains('=') && !flag(arg) {
            rest.next();
        }
    }
    if !["artifact", "answer"].contains(&result.as_str()) {
        return Err("invalid --output".into());
    }
    Ok(result)
}

fn file_content(
    route: &Route,
    cli: &Client,
    issuer: &str,
    relative: Option<&str>,
) -> Result<String, String> {
    let mut request = cli.grant("read", issuer, None)?;
    request["args"] = json!(["read"]);
    request["cwd"] = Value::Null;
    request["run_request"] = Value::Null;
    request["operation_id"] = Value::Null;
    request["artifact"] = json!(relative.is_none());
    request["file"] = json!(relative);
    let result = cli.exchange(&sign(&request, &route.authority)?)?;
    validate_view(route, &result, &cli.id)?;
    Ok(text(&result, "content")?.to_owned())
}

fn print_bytes(content: &str) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    out.write_all(content.as_bytes())
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())
}

fn output(
    route: &Route,
    cli: &Client,
    issuer: &str,
    result: &Value,
    mode: String,
) -> Result<(), String> {
    if mode == "answer" && !result["operation_result"].is_null() {
        print_bytes(&file_content(route, cli, issuer, None)?)?;
    } else {
        println!(
            "{}",
            json!({"run_id":route.id,"status":result["manifest"]["status"].as_str().unwrap_or("running"),
            "run_root":null,"handoff_path":null})
        );
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
    if result["operation_result"]["ok"] == false {
        return Err(
            "remote operation failed; inspect read RUN_ID manifest.json and retained diagnostics"
                .into(),
        );
    }
    Ok(())
}

fn forwarded_args(args: &[String], target: usize) -> Result<(Vec<String>, bool), String> {
    let op = args[0].as_str();
    let mut forwarded = vec![op.into()];
    let mut detach = false;
    let mut index = 1;
    while index < args.len() {
        if index == target {
            index += 1;
            continue;
        }
        if args[index] == "--store" {
            index += 2;
            continue;
        }
        let arg = &args[index];
        if arg == "--detach" && op == "resume" {
            if detach {
                return Err("duplicate --detach".into());
            }
            detach = true;
        } else if arg == "--task-file" {
            index += 1;
            let path = args.get(index).ok_or("missing --task-file")?;
            let mut bytes = Vec::new();
            File::open(path)
                .and_then(|f| f.take(LIMIT + 1).read_to_end(&mut bytes))
                .map_err(|e| format!("read local task file: {e}"))?;
            if bytes.len() as u64 > LIMIT {
                return Err("task file exceeds remote limit".into());
            }
            forwarded.extend([
                "--task".into(),
                String::from_utf8(bytes).map_err(|_| "task requires UTF-8")?,
            ]);
        } else {
            forwarded.push(arg.clone());
            if arg == "--" {
                forwarded.extend(
                    args[index + 1..]
                        .iter()
                        .enumerate()
                        .filter(|(offset, _)| index + 1 + offset != target)
                        .map(|(_, arg)| arg.clone()),
                );
                break;
            }
            if arg.starts_with('-') && !arg.contains('=') && !flag(arg) {
                index += 1;
                forwarded.push(
                    args.get(index)
                        .ok_or_else(|| format!("missing {arg}"))?
                        .clone(),
                );
            }
        }
        index += 1;
    }
    Ok((forwarded, detach))
}

fn ordinary(route: &Route, args: &[String], target: usize) -> Result<(), String> {
    let context = verified_root(args)?;
    let issuer = text(&context, "caller_thread_id")?;
    let op = args[0].as_str();
    if op == "path" {
        return Err(
            "remote artifacts have no local filesystem path; use read RUN_ID [RELATIVE_FILE]"
                .into(),
        );
    }
    if op == "tier" {
        return Err("remote live tier control is unsupported; resume --service-tier retains launch/resume support".into());
    }
    let (forwarded, detach) = forwarded_args(args, target)?;
    if matches!(op, "read" | "wait") {
        if forwarded.len() > 2
            || (forwarded.len() == 2 && (op == "wait" || forwarded[1].starts_with('-')))
        {
            return Err("expected read RUN_ID [RELATIVE_FILE] or wait RUN_ID".into());
        }
        let pending = route.pending("resume", issuer)?;
        let envelope = pending.as_ref().unwrap_or(&route.envelope);
        let mut cli = route.cli(&envelope["grant"])?;
        cli.negotiate(issuer, "read", false)?;
        let mut pending_accepted = false;
        let mut result = if pending.is_some() {
            let observed = cli.observe(issuer).map_err(|e| recovery_error(route, e))?;
            if observed["accepted"] == false
                && observed["status"] == "not_found"
                && observed["request_id"] == cli.id
            {
                // Ordinary observation cannot create even a previously intended
                // resume turn. Only a repeated explicit resume submits it.
                cli = route.cli(&route.envelope["grant"])?;
                observe(route, &cli, &context, &route.envelope)?
            } else {
                validate_view(route, &observed, &cli.id)?;
                pending_accepted = true;
                observed
            }
        } else {
            observe(route, &cli, &context, envelope)?
        };
        if op == "wait" {
            result = wait(route, &cli, issuer, result)?;
        }
        if forwarded.get(1).map(String::as_str) == Some("manifest.json") {
            if result["manifest"].is_null() {
                return Err("remote manifest not yet available; run ID retained".into());
            }
            println!("{}", result["manifest"]);
        } else if op == "wait" {
            output(route, &cli, issuer, &result, "artifact".into())?;
        } else {
            print_bytes(&file_content(
                route,
                &cli,
                issuer,
                forwarded.get(1).map(String::as_str),
            )?)?;
        }
        // A successful ordinary observation settles an uncertain resume for a
        // full-access caller. Tightened-policy reads remain state-write-free.
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        if execute_policy(&context, issuer).is_ok() {
            let _lock = lock(&route.dir.join("controller.lock"))?;
            if let Some(envelope) = pending
                && pending_accepted
            {
                route.mark_delivered(&envelope["grant"])?;
            }
            if !result["control_target"].is_null() {
                route.observe_controls(issuer, &result["control_target"])?;
            }
        }
        return Ok(());
    }
    if !matches!(op, "resume" | "steer" | "interrupt") {
        return Err("unsupported remote operation".into());
    }
    if op != "interrupt" {
        execute_policy(&context, issuer)?;
    }
    let _lock = lock(&route.dir.join("controller.lock"))?;
    let pending = route.pending(op, issuer)?;
    let mut base = route.cli(&route.envelope["grant"])?;
    base.args = forwarded.clone();
    base.cwd = None;
    base.detach = detach;
    base.run_request = if op == "resume" {
        Some(base.id.clone())
    } else {
        None
    };
    let mut grant = base.grant(
        if op == "resume" { "submit" } else { op },
        issuer,
        (op != "interrupt").then(|| execution(&context)).as_ref(),
    )?;
    let unfinished = match pending.as_ref() {
        Some(envelope)
            if !route.delivered(&envelope["grant"])?
                && (op == "resume" || !route.superseded(envelope)?) =>
        {
            Some(envelope)
        }
        _ => None,
    };
    let envelope = if let Some(retained) = unfinished {
        grant["request_id"] = retained["grant"]["request_id"].clone();
        grant["operation_id"] = retained["grant"]["operation_id"].clone();
        grant["target"] = retained["grant"]["target"].clone();
        if logical(&grant) != logical(&retained["grant"]) {
            return Err("uncertain operation already binds different payload/policy; use ordinary read RUN_ID manifest.json to settle it before a new operation".into());
        }
        retained.clone()
    } else {
        if op == "resume" {
            grant["request_id"] = json!(random()?);
            let metadata = base.metadata(issuer)?;
            let retained = observe(route, &base, &context, &route.envelope)?;
            require_resume_capabilities(
                &metadata,
                &retained,
                &route.envelope["grant"],
                &base.args,
            )?;
            if retained["manifest"]["status"] == "running" {
                return Err("run is still active; use read/wait, not resume".into());
            }
        } else {
            base.negotiate(issuer, op, false)?;
            grant["operation_id"] = json!(random()?);
            grant["target"] = base.observe(issuer)?["control_target"].clone();
        }
        validate_grant(&grant)?;
        let envelope = sign(&grant, &route.authority)?;
        let path = route.dir.join(format!("pending-{op}.json"));
        lifecycle::save_json(&path, &envelope)?;
        sync_dir(&route.dir)?;
        envelope
    };
    let mut cli = route.cli(&envelope["grant"])?;
    cli.detach = base.detach;
    let result = if op == "resume" {
        // Retained pending submit is observed before any possible resubmission.
        observe(route, &cli, &context, &envelope)?
    } else {
        let receipt = cli.operation_receipt(issuer)?;
        if receipt["status"] == "not_found" {
            let mut grant = envelope["grant"].clone();
            let time = now()?;
            grant["issued_at"] = json!(time);
            grant["expires_at"] = json!(time + VALIDITY);
            cli.exchange(&sign(&grant, &route.authority)?)
                .map_err(|e| recovery_error(route, e))?
        } else {
            let mut intent = logical(&envelope["grant"]);
            intent.as_object_mut().unwrap().remove("target");
            if receipt["intent"] != intent || receipt["target"] != envelope["grant"]["target"] {
                return Err("retained control differs from signed intent/epoch".into());
            }
            receipt
        }
    };
    drop(_lock);
    if op == "resume" {
        let result = if cli.detach {
            result
        } else {
            wait(route, &cli, issuer, result)?
        };
        let default = result["manifest"]["output"].as_str().unwrap_or("artifact");
        output(
            route,
            &cli,
            issuer,
            &result,
            output_mode(&base.args, default)?,
        )?;
    } else {
        let native = &result["result"]["receipt"];
        if native.is_null() {
            println!("{result}");
        } else {
            println!("{native}");
        }
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        if result["result"]["status"] != "acknowledged" {
            return Err("remote control outcome is unknown/failed; retained operation will not be reissued; inspect read RUN_ID manifest.json".into());
        }
    }
    let _lock = lock(&route.dir.join("controller.lock"))?;
    route.mark_delivered(&envelope["grant"])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).into()).collect()
    }

    #[test]
    fn authorization_rejection_precedes_missing_routing_arguments() {
        for command in [
            "resume",
            "steer",
            "interrupt",
            "read",
            "path",
            "tier",
            "wait",
        ] {
            let mut args = strings(&[command]);
            let result =
                dispatch_using(&mut args, |_| Err("nested delegation is forbidden".into()));
            assert_eq!(
                result,
                Some(Err("nested delegation is forbidden".into())),
                "{command}"
            );
            assert_eq!(
                dispatch_using(&mut args, |_| Ok(())),
                Some(Err("missing RUN_ID".into())),
                "{command}"
            );
        }
    }

    struct Fixture {
        root: PathBuf,
        cli: Client,
        envelope: Value,
    }
    impl Fixture {
        fn new() -> Self {
            // Keep the native temp path: Windows canonicalization introduces a
            // verbatim prefix that external OpenSSH tools need not understand.
            let root = std::env::temp_dir().join(format!("pira-routing-{}", random().unwrap()));
            crate::private_dir(&root).unwrap();
            let authority = root.join("synthetic-key");
            assert!(
                Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                    .arg(&authority)
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(authority.is_absolute() && authority.is_file());
            write_new(
                &appended(&authority, ".owner.json"),
                &json!({"version":VERSION,"issuer":"root",
                "public":public_key(&authority).unwrap()}),
            )
            .unwrap();
            let cli = Client {
                endpoint: json!({"host":"fixture","user":"fixture","endpoint_id":"a".repeat(64),
                    "exe":"/opt/pira_team","backend":"/opt/codex","store":"/srv/team"}),
                authority,
                state: root.clone(),
                id: "initial".into(),
                run_request: None,
                operation: None,
                cwd: Some("/srv/workspace".into()),
                detach: false,
                artifact: false,
                file: None,
                args: strings(&["run", "--task", "fixture", "--completion-gate", "done"]),
            };
            let grant = cli
                .grant(
                    "submit",
                    "root",
                    Some(&json!({"caller_thread_id":"root","approval_policy":"never",
                "sandbox_policy":{"type":"danger-full-access"}})),
                )
                .unwrap();
            let envelope = sign(&grant, &cli.authority).unwrap();
            Self {
                root,
                cli,
                envelope,
            }
        }
        fn route(&self) -> Route {
            retain_at(&self.root, &self.cli, &self.envelope).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn generated_identity_precedes_literal_task_terminator() {
        let local = std::env::temp_dir().join("pira-routing-local");
        let authority = local.join("synthetic");
        let mut args = strings(&[
            "run",
            "--host",
            "fixture",
            "--remote-user",
            "fixture",
            "--remote-endpoint-id",
            &"a".repeat(64),
            "--remote-exe",
            "/opt/pira_team",
            "--remote-backend",
            "/opt/codex",
            "--remote-store",
            "/srv/team",
            "--remote-cwd",
            "/srv/work",
            "--authority-key",
            authority.to_str().unwrap(),
            "--completion-gate",
            "done",
            "--",
            "--host=literal --request-id=literal",
        ]);
        set_launch_identity(&mut args, &local, "stable".into());
        let cli = Client::parse(&args).unwrap();
        assert_eq!(cli.id, "stable");
        assert_eq!(cli.state, local.join(".remote-controller"));
        assert_eq!(cli.authority, authority);
        for option in ["--authority-key", "--remote-state"] {
            let mut relative = args.clone();
            let value = relative.iter().position(|arg| arg == option).unwrap() + 1;
            relative[value] = "relative-local-path".into();
            assert_eq!(
                Client::parse(&relative).err().as_deref(),
                Some("--remote-state and --authority-key are explicit local absolute paths"),
                "{option}"
            );
        }
        assert_eq!(
            cli.args.last().unwrap(),
            "--host=literal --request-id=literal"
        );
    }

    #[test]
    fn normal_control_forwarding_preserves_literal_option_values() {
        let args = strings(&[
            "resume",
            "id",
            "--task",
            "--detach",
            "--completion-gate",
            "--store",
            "--store",
            "/tmp/local",
        ]);
        let (forwarded, detach) = forwarded_args(&args, 1).unwrap();
        assert!(!detach);
        assert_eq!(
            forwarded,
            strings(&[
                "resume",
                "--task",
                "--detach",
                "--completion-gate",
                "--store"
            ])
        );
        let args = strings(&[
            "resume",
            "--completion-gate",
            "done",
            "--",
            "id",
            "--host literal",
        ]);
        assert_eq!(
            forwarded_args(&args, 4).unwrap().0,
            strings(&[
                "resume",
                "--completion-gate",
                "done",
                "--",
                "--host literal"
            ])
        );
    }

    #[test]
    fn local_task_file_is_materialized_before_signing_without_remote_path_authority() {
        let fixture = Fixture::new();
        let path = fixture.root.join("task");
        fs::write(&path, "literal λ\n--detach").unwrap();
        let args = strings(&[
            "steer",
            "id",
            "--task-file",
            path.to_str().unwrap(),
            "--completion-gate",
            "done",
        ]);
        let (forwarded, _) = forwarded_args(&args, 1).unwrap();
        assert_eq!(
            control_task(&forwarded).unwrap(),
            ("literal λ\n--detach".into(), "done".into())
        );
    }

    #[test]
    fn durable_route_reopens_same_allocation_without_native_manifest_or_duplicate() {
        let fixture = Fixture::new();
        let route = fixture.route();
        assert_eq!(
            route.id,
            format!(
                "remote-{}",
                fixture.envelope["grant"]["nonce"].as_str().unwrap()
            )
        );
        assert!(!fixture.root.join(&route.id).exists());
        assert_eq!(fixture.route().id, route.id);
        assert_eq!(fs::read_dir(fixture.root.join(ROUTES)).unwrap().count(), 1);
        let mut changed = fixture.envelope["grant"].clone();
        changed["args"] = json!(["run", "--task", "changed"]);
        let changed = sign(&changed, &fixture.cli.authority).unwrap();
        assert!(retain_at(&fixture.root, &fixture.cli, &changed).is_err());
    }

    #[test]
    fn route_signature_and_main_scope_reject_tampering_before_transport() {
        let fixture = Fixture::new();
        let route = fixture.route();
        assert!(Route::load(&fixture.root, &route.id, "other-root").is_err());
        let path = route.dir.join("route.json");
        let mut record = read(&path).unwrap();
        record["envelope"]["grant"]["endpoint"]["host"] = json!("untrusted");
        lifecycle::save_json(&path, &record).unwrap();
        assert!(Route::load(&fixture.root, &route.id, "root").is_err());
    }

    #[test]
    fn pending_resume_and_delivery_markers_bind_exact_signed_operation() {
        let fixture = Fixture::new();
        let route = fixture.route();
        let mut grant = fixture.envelope["grant"].clone();
        grant["args"] = json!(["resume"]);
        grant["request_id"] = json!("next");
        grant["run_request"] = json!("initial");
        grant["cwd"] = Value::Null;
        let pending = sign(&grant, &fixture.cli.authority).unwrap();
        write_new(&route.dir.join("pending-resume.json"), &pending).unwrap();
        assert_eq!(route.pending("resume", "root").unwrap().unwrap(), pending);
        assert!(!route.delivered(&grant).unwrap());
        route.mark_delivered(&grant).unwrap();
        route.mark_delivered(&grant).unwrap();
        assert!(route.delivered(&grant).unwrap());
        grant["args"] = json!(["resume", "--task", "changed"]);
        assert!(route.delivered(&grant).is_err());
        grant["run_request"] = json!("another-run");
        let invalid = sign(&grant, &fixture.cli.authority).unwrap();
        lifecycle::save_json(&route.dir.join("pending-resume.json"), &invalid).unwrap();
        assert!(route.pending("resume", "root").is_err());
    }

    #[test]
    fn partial_claim_is_not_authoritative_absence_and_local_collisions_fail_closed() {
        let fixture = Fixture::new();
        let mut grant = fixture.envelope["grant"].clone();
        grant["endpoint"]["store"] = json!(fixture.root.to_str().unwrap());
        assert!(unaccepted(&grant).unwrap());
        directory(&fixture.root.join(".remote-v2")).unwrap();
        directory(&fixture.root.join(".remote-v2/initial")).unwrap();
        assert!(!unaccepted(&grant).unwrap());
        let route = fixture.route();
        crate::private_dir(&fixture.root.join(&route.id)).unwrap();
        assert!(Route::load(&fixture.root, &route.id, "root").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_route_directory_never_authorizes_endpoint_selection() {
        let fixture = Fixture::new();
        let route = fixture.route();
        let moved = fixture.root.join("retained-route");
        fs::rename(&route.dir, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &route.dir).unwrap();
        assert!(Route::load(&fixture.root, &route.id, "root").is_err());
    }

    #[test]
    fn observing_new_epoch_retires_unknown_control_without_claiming_delivery() {
        let fixture = Fixture::new();
        let route = fixture.route();
        let mut cli = route.cli(&fixture.envelope["grant"]).unwrap();
        cli.args = strings(&["interrupt"]);
        cli.cwd = None;
        cli.operation = Some("stop".into());
        let mut grant = cli.grant("interrupt", "root", None).unwrap();
        let target = json!({"revision":1,"subtree_epoch":0,"control_epoch":"old","turn_id":"old"});
        grant["target"] = target.clone();
        let envelope = sign(&grant, &fixture.cli.authority).unwrap();
        write_new(&route.dir.join("pending-interrupt.json"), &envelope).unwrap();
        route.observe_controls("root", &target).unwrap();
        assert!(!route.superseded(&envelope).unwrap());
        let mut next = target;
        next["revision"] = json!(2);
        next["control_epoch"] = json!("new");
        route.observe_controls("root", &next).unwrap();
        assert!(route.superseded(&envelope).unwrap());
        assert!(!route.delivered(&grant).unwrap());
        assert_eq!(
            route.pending("interrupt", "root").unwrap().unwrap(),
            envelope
        );
    }
}
