//! Bootstrap only after verified root authorization; SSH enrolls public data,
//! while all subsequent work still requires the existing bounded signed grants.
use super::*;

fn option(args: &[String], name: &str) -> Result<Option<String>, String> {
    let mut value = None;
    for token in cli::tokens(args.get(1..).unwrap_or_default()) {
        if token.raw == "--" {
            break;
        }
        if token.name == name {
            let next = token
                .value
                .ok_or_else(|| format!("missing {name}"))?
                .to_owned();
            crate::set_once(&mut value, next, name)?;
        }
    }
    Ok(value)
}

fn exchange(
    host: &str,
    command: &str,
    request: &Value,
    diagnostics: &SharedDiagnostics,
) -> Result<Value, String> {
    let mut child = ssh_command(host, None, command)
        .spawn()
        .map_err(|e| format!("start SSH discovery: {e}"))?;
    let bytes = transport::exchange(
        &mut child,
        &serde_json::to_vec(request).map_err(|e| e.to_string())?,
        || crate::CANCELLED.load(std::sync::atomic::Ordering::SeqCst),
        &mut diagnostics.borrow_mut(),
    )
        .map_err(|e|format!("installed remote Team unavailable/incompatible: {e}; run PIRA setup/update on the server; no worker submitted"))?;
    let reply:Value=serde_json::from_slice(&bytes).map_err(|_|"installed Team lacks structured remote setup; update PIRA Team on the server; no worker submitted")?;
    if let Some(error) = reply["error"].as_str() {
        return Err(format!("remote setup: {error}; no worker submitted"));
    }
    Ok(reply)
}

const DISCOVER: &str = r#"p=$(command -v pira_team 2>/dev/null) || p="$HOME/.local/bin/pira_team"; if [ ! -x "$p" ]; then printf '%s\n' 'PIRA Team not found; run PIRA setup on the server' >&2; exit 127; fi; exec "$p" __remote-info"#;

pub(super) fn prepare(
    args: &mut Vec<String>,
    root: &Path,
    context: &Value,
    diagnostics: &SharedDiagnostics,
) -> Result<(), String> {
    // The complete explicit endpoint interface is unchanged, including its independent enrollment.
    if has_option(args, "--remote-endpoint-id") {
        return Ok(());
    }
    let host_name = option(args, "--host")?.ok_or("missing --host")?;
    host(&host_name)?;
    let issuer = text(context, "caller_thread_id")?;
    execute_policy(context, issuer)?;
    let command = option(args, "--remote-exe")?
        .map(|p| format!("exec {} __remote-info", quote(&p)))
        .unwrap_or_else(|| DISCOVER.into());
    let info = exchange(&host_name, &command, &Value::Null, diagnostics)?;
    if info["protocol"] != VERSION
        || info["setup"] != 1
        || info["version"] != env!("CARGO_PKG_VERSION")
    {
        return Err("incompatible installed Team; update PIRA Team on the server to the local version; no automatic binary installation is performed".into());
    }
    let user = text(&info, "user")?;
    account_name(user)?;
    if let Some(expected) = option(args, "--remote-user")?
        && expected != user
    {
        return Err("--remote-user differs from authenticated SSH account".into());
    }
    signing_support()?;
    let selected = option(args, "--authority-key")?;
    let authority = if let Some(path) = selected {
        PathBuf::from(path)
    } else {
        let base = root.join(".remote-bootstrap");
        directory(&base)?;
        let owned = base.join(issuer);
        directory(&owned)?;
        let _lock = lock(&owned.join("bootstrap.lock"))?;
        let authority = owned.join("key/authority");
        if !authority.exists() {
            // Generate in a disposable directory, then publish the complete key bundle.
            // A killed keygen leaves only an unused staging directory, not a broken active key.
            let stage = owned.join(random()?);
            directory(&stage)?;
            let temporary = stage.join("authority");
            let status = Command::new(ssh_keygen()?)
                .args(["-q", "-t", "ed25519", "-N", "", "-C", issuer, "-f"])
                .arg(&temporary)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|_| "OpenSSH ssh-keygen is required")?;
            if !status.success() {
                return Err("automatic remote authority generation failed".into());
            }
            let public = public_key(&temporary)?;
            write_new(
                &appended(&temporary, ".owner.json"),
                &json!({"version":VERSION,"issuer":issuer,"public":public}),
            )?;
            // The directory rename is the atomic publication boundary.
            fs::rename(&stage, owned.join("key"))
                .map_err(|e| format!("publish bootstrap authority: {e}"))?;
            sync_dir(&owned)?;
        }
        owned.join("key/authority")
    };
    existing(&authority, false)?;
    let owner = read(&appended(&authority, ".owner.json"))?;
    let public = public_key(&authority)?;
    if owner != json!({"version":VERSION,"issuer":issuer,"public":public}) {
        return Err("bootstrap authority is not bound to this verified main".into());
    }
    let endpoint = exchange(
        &host_name,
        &format!("exec {} __remote-setup", quote(text(&info, "exe")?)),
        &json!({"issuer":issuer,"public":public,"version":env!("CARGO_PKG_VERSION"),"protocol":VERSION,
        "backend":option(args,"--remote-backend")?,"store":option(args,"--remote-store")?}),
        diagnostics,
    )?;
    let defaults = [
        ("--remote-user", endpoint["user"].clone()),
        ("--remote-endpoint-id", endpoint["endpoint_id"].clone()),
        ("--remote-exe", endpoint["exe"].clone()),
        ("--remote-backend", endpoint["backend"].clone()),
        ("--remote-store", endpoint["store"].clone()),
        ("--authority-key", json!(authority)),
        ("--remote-cwd", info["home"].clone()),
    ];
    // Discovery canonicalizes a selected installed executable; it never copies/updates it.
    let mut result = Vec::new();
    {
        let mut rest = cli::tokens(args);
        while let Some(token) = rest.next() {
            if token.raw == "--" {
                result.extend(token.words.iter().cloned());
                result.extend(rest.flat_map(|token| token.words.iter().cloned()));
                break;
            }
            if token.name != "--remote-exe" {
                result.extend(token.words.iter().cloned());
            }
        }
    }
    for (name, value) in defaults {
        if !has_option(&result, name) {
            result.splice(
                1..1,
                [
                    name.into(),
                    value
                        .as_str()
                        .ok_or("invalid bootstrap endpoint response")?
                        .into(),
                ],
            );
        }
    }
    *args = result;
    Ok(())
}

fn state_root() -> Result<PathBuf, String> {
    if let Some(store) = std::env::var_os("PIRA_TEAM_DIR").filter(|s| !s.is_empty()) {
        let store = PathBuf::from(store);
        absolute(store.to_str().ok_or("PIRA_TEAM_DIR requires UTF-8")?)?;
        return Ok(store.join(".remote-control-v2"));
    }
    if let Some(base) = std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        let base = PathBuf::from(base);
        absolute(base.to_str().ok_or("XDG_DATA_HOME requires UTF-8")?)?;
        return Ok(base.join("pira-team-control-v2"));
    }
    Ok(current_account()?
        .1
        .join(".local/share/pira-team-control-v2"))
}
fn create_state() -> Result<PathBuf, String> {
    let root = state_root()?;
    let parent = root
        .ancestors()
        .find(|path| path.exists())
        .ok_or("missing control-root ancestor")?;
    ancestors(parent)?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&root)
        .map_err(|e| format!("create private remote control root: {e}"))?;
    existing(&root, true)?;
    root.canonicalize().map_err(|e| e.to_string())
}
pub(super) fn trust_paths(exe: &Path, id: &str) -> Result<(PathBuf, PathBuf), String> {
    endpoint_id(id)?;
    let preferred = state_root()?;
    let legacy = current_account()?
        .1
        .join(".local/share/pira-team-control-v2");
    for root in [preferred, legacy] {
        let managed = root.join(id);
        if managed.exists() {
            existing(&root, true)?;
            existing(&managed, true)?;
            return Ok((
                managed.join("allowed-signers"),
                managed.join("endpoint.json"),
            ));
        }
    }
    Ok((
        appended(exe, ".allowed-signers"),
        appended(exe, ".remote-endpoint.json"),
    ))
}
fn backend(selected: Option<&str>) -> Result<PathBuf, String> {
    backend_at(
        selected,
        &current_account()?.1,
        std::env::var_os("PATH").as_deref(),
    )
}
fn backend_at(
    selected: Option<&str>,
    home: &Path,
    search: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, String> {
    let managed = home.join(".local/bin/.pira-codex/bin/codex");
    let marker = home.join(".local/bin/.pira-codex/selected");
    let candidate = if let Some(selected) = selected {
        PathBuf::from(selected)
    } else {
        let mut candidates = Vec::new();
        if marker.exists() {
            trusted_public(&marker)?;
            let mut selection = Vec::new();
            File::open(&marker)
                .and_then(|file| file.take(12).read_to_end(&mut selection))
                .map_err(|e| e.to_string())?;
            if selection != b"managed-v1\n" {
                return Err(
                    "invalid setup-managed Codex selection; rerun PIRA setup on server".into(),
                );
            }
            let selected = managed.canonicalize().map_err(
                |_| "selected setup-managed Codex is missing; rerun PIRA setup on server",
            )?;
            executable(&selected)?;
            return Ok(selected);
        }
        candidates.extend(search.into_iter().flat_map(|p| {
            std::env::split_paths(&p)
                .map(|dir| dir.join("codex"))
                .collect::<Vec<_>>()
        }));
        candidates.extend([home.join(".local/bin/codex"), managed]);
        candidates
            .into_iter()
            .find(|p| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    p.metadata()
                        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                }
                #[cfg(not(unix))]
                {
                    p.is_file()
                }
            })
            .ok_or("Codex CLI not found; rerun PIRA setup on the server")?
    };
    let path = candidate
        .canonicalize()
        .map_err(|_| "Codex CLI not found; rerun PIRA setup on the server")?;
    executable(&path)?;
    Ok(path)
}
fn setup(request: Value) -> Result<Value, String> {
    if !cfg!(target_os = "linux") {
        return Err("installed remote Team requires Linux".into());
    }
    if request["version"] != env!("CARGO_PKG_VERSION") || request["protocol"] != VERSION {
        return Err("incompatible installed Team; update PIRA Team on the server".into());
    }
    let issuer = text(&request, "issuer")?;
    profile::validate_session_id(issuer)?;
    let public = text(&request, "public")?;
    if public.len() > 100
        || !public.starts_with("ssh-ed25519 ")
        || public.split_whitespace().count() != 2
        || public[12..]
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && !b"+/=".contains(&b))
    {
        return Err("invalid public launcher enrollment".into());
    }
    let (exe, _, user) = endpoint_identity()?;
    signing_support()?;
    let backend = backend(request["backend"].as_str())?;
    let root = create_state()?;
    let _lock = lock(&root.join("setup.lock"))?;
    // This hash is an index only, never authority: exact binding and signatures
    // are checked independently; even a collision fails closed.
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (issuer, public, &exe).hash(&mut hash);
    let index = root.join(format!("key-{:016x}.json", hash.finish()));
    let id = if index.exists() {
        existing(&index, false)?;
        let retained = read(&index)?;
        if retained["issuer"] != issuer
            || retained["public"] != public
            || retained["exe"] != json!(exe)
        {
            return Err("enrollment index binding mismatch; refusing replacement".into());
        }
        text(&retained, "endpoint_id")?.to_owned()
    } else {
        random()?
    };
    endpoint_id(&id)?;
    let dir = root.join(&id);
    let descriptor = json!({"version":VERSION,"endpoint_id":id,"exe":exe,"user":user});
    let line = format!("{issuer} namespaces=\"{NAMESPACE}\" {public}\n");
    if dir.exists() {
        existing(&dir, true)?;
        if public_record(&dir.join("endpoint.json"))? != descriptor {
            return Err("existing endpoint binding changed; refusing overwrite".into());
        }
        trusted_public(&dir.join("allowed-signers"))?;
        if fs::read_to_string(dir.join("allowed-signers")).map_err(|e| e.to_string())? != line {
            return Err("existing enrollment changed; refusing overwrite".into());
        }
    } else {
        let stage = root.join(format!("stage-{}", random()?));
        directory(&stage)?;
        write_new(&stage.join("endpoint.json"), &descriptor)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(stage.join("allowed-signers"))
            .map_err(|e| e.to_string())?;
        file.write_all(line.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        sync_dir(&stage)?;
        fs::rename(&stage, &dir).map_err(|e| e.to_string())?;
        sync_dir(&root)?;
    }
    if !index.exists() {
        write_new(
            &index,
            &json!({"issuer":issuer,"public":public,"exe":exe,"endpoint_id":id}),
        )?;
    }
    let store = request["store"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("runs"));
    absolute(store.to_str().ok_or("remote store requires UTF-8")?)?;
    canonical_path(&store, false)?;
    ancestors(if store.exists() {
        &store
    } else {
        store.parent().ok_or("missing store parent")?
    })?;
    Ok(json!({"endpoint_id":id,"exe":exe,"user":user,"backend":backend,"store":store}))
}
pub(super) fn serve_setup() -> Result<(), String> {
    let result = match input().and_then(setup) {
        Ok(value) => value,
        Err(error) => json!({"error":error}),
    };
    println!("{result}");
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn managed_codex_is_found_without_shell_activation_and_selection_beats_path() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("managed-codex-{}", random().unwrap()));
        let managed = root.join(".local/bin/.pira-codex/bin/codex");
        fs::create_dir_all(managed.parent().unwrap()).unwrap();
        fs::write(&managed, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&managed, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            backend_at(None, &root, Some(std::ffi::OsStr::new("/nonexistent"))).unwrap(),
            managed
        );
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("codex"), b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(other.join("codex"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            root.join(".local/bin/.pira-codex/selected"),
            b"managed-v1\n",
        )
        .unwrap();
        assert_eq!(
            backend_at(None, &root, Some(other.as_os_str())).unwrap(),
            managed
        );
        fs::remove_file(&managed).unwrap();
        assert!(
            backend_at(None, &root, Some(other.as_os_str()))
                .unwrap_err()
                .contains("selected setup-managed Codex is missing")
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
