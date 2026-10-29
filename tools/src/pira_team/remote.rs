//! SSH is transport, not delegation authority. Only independently enrolled launcher
//! signatures authorize root operations. Tasks on the same OS user are not isolated
//! from a compromised launcher/key/store; no signing secret is transferred remotely.
use crate::{lifecycle, profile, storage};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(super) mod routing;

const VERSION: u64 = 2;
const NAMESPACE: &str = "pira-team-remote-v2";
const SCOPE: &str = "root-main-host-pinned";
const LIMIT: u64 = 16 * 1024 * 1024;
const VALIDITY: u64 = 300;
const CAPABILITIES: &[&str] = &[
    "signed-delegation",
    "host-pinned-subteam",
    "durable-submit",
    "read",
    "resume",
    "steer",
    "interrupt",
];

fn capability_metadata() -> Value {
    json!({"protocol":VERSION,"scope":SCOPE,"capabilities":CAPABILITIES})
}

fn require_capabilities(metadata: &Value, operation: &str, subteam: bool) -> Result<(), String> {
    if metadata["protocol"] != VERSION {
        return Err("remote capability protocol mismatch; no operation submitted".into());
    }
    let available = metadata["capabilities"]
        .as_array()
        .ok_or("remote capabilities missing/malformed; no operation submitted")?;
    if available.iter().any(|value| !value.is_string()) {
        return Err("malformed remote capabilities; no operation submitted".into());
    }
    let mut required = vec!["signed-delegation", "read"];
    match operation {
        "run" => required.push("durable-submit"),
        "resume" => required.extend(["durable-submit", "resume"]),
        "steer" => required.push("steer"),
        "interrupt" => required.push("interrupt"),
        "read" | "attach" | "remote-check" => {}
        _ => return Err("unsupported capability negotiation operation".into()),
    }
    if subteam {
        required.push("host-pinned-subteam");
    }
    for name in required {
        if !available.iter().any(|value| value.as_str() == Some(name)) {
            return Err(format!(
                "remote capabilities missing required {name}; no operation submitted"
            ));
        }
    }
    if metadata["scope"] != SCOPE {
        return Err("remote capability scope mismatch; no operation submitted".into());
    }
    Ok(())
}

fn require_resume_capabilities(
    metadata: &Value,
    retained: &Value,
    original: &Value,
    args: &[String],
) -> Result<(), String> {
    require_capabilities(metadata, "read", false)?;
    let run = text(retained, "run_id")?;
    storage::validate_run_id(run)?;
    let manifest = &retained["manifest"];
    // The authenticated read resolves the original request, including a
    // same-key retry whose fresh nonce was reconciled to an earlier accepted run.
    if retained["accepted"] != true
        || retained["request_id"] != original["request_id"]
        || manifest["run_id"] != run
        || manifest["creator_main_session_id"] != original["issuer"]
        || manifest.get("parent_run_id") != Some(&Value::Null)
    {
        return Err("authenticated retained topology is missing or has mismatched root/run identity; no resume submitted".into());
    }
    let retained_subteam = manifest["allow_subteam"].as_bool().ok_or(
        "unknown retained topology; explicit allow_subteam state is required before resume",
    )?;
    let subteam = retained_subteam
        || requests_subteam(args)
        || requests_subteam(&strings(&original["args"])?);
    require_capabilities(metadata, "resume", subteam)
}

fn requests_subteam(args: &[String]) -> bool {
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break;
        }
        if arg == "--allow-subteam" {
            return true;
        }
        if arg.starts_with('-') && !arg.contains('=') && !flag(arg) {
            rest.next();
        }
    }
    false
}

#[derive(Clone)]
struct Delegation {
    context: Value,
    backend: PathBuf,
}
thread_local! { static DELEGATION: RefCell<Option<Delegation>> = const { RefCell::new(None) }; }
pub(super) fn delegated_context() -> Option<Value> {
    DELEGATION.with(|slot| slot.borrow().as_ref().map(|d| d.context.clone()))
}
pub(super) fn delegated_backend() -> Option<PathBuf> {
    DELEGATION.with(|slot| slot.borrow().as_ref().map(|d| d.backend.clone()))
}
fn install(grant: &Value) -> Result<(), String> {
    let context = if grant["context"].is_null() {
        json!({"caller_thread_id":grant["issuer"]})
    } else {
        grant["context"].clone()
    };
    let backend = PathBuf::from(text(&grant["endpoint"], "backend")?);
    DELEGATION.with(|slot| *slot.borrow_mut() = Some(Delegation { context, backend }));
    Ok(())
}

pub(super) fn dispatch(args: &mut [String]) -> Option<Result<(), String>> {
    match args.first().map(String::as_str) {
        Some("__remote-serve") => Some(if args.len() == 1 {
            serve()
        } else {
            Err("remote endpoint takes no path or trust options".into())
        }),
        Some("__remote-supervise") => Some(if args.len() == 2 {
            supervise(Path::new(&args[1]))
        } else {
            Err("invalid supervisor invocation".into())
        }),
        Some("remote-keygen") => Some(keygen(args)),
        Some("resolve") => Some(routing::resolve(args)),
        Some("run") if has_host(args) && !has_option(args, "--request-id") => {
            Some(routing::launch(args))
        }
        _ if has_host(args) => Some(client(args)),
        _ => routing::dispatch(args),
    }
}
fn has_option(args: &[String], option: &str) -> bool {
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break;
        }
        if arg.split('=').next() == Some(option) {
            return true;
        }
        if arg.starts_with('-') && !arg.contains('=') && !flag(arg) {
            rest.next();
        }
    }
    false
}
fn flag(arg: &str) -> bool {
    matches!(
        arg,
        "--detach"
            | "--artifact"
            | "--inject-review"
            | "--inject-implement"
            | "--inject-write"
            | "--inject-fig"
            | "--allow-subteam"
            | "--allow-fix"
            | "--code-review"
            | "-r"
            | "--recursive"
    )
}
fn has_host(args: &[String]) -> bool {
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break;
        }
        if arg == "--host" || arg.starts_with("--host=") {
            return true;
        }
        if arg.starts_with('-') && !arg.contains('=') && !flag(arg) {
            rest.next();
        }
    }
    false
}
fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value[name]
        .as_str()
        .ok_or_else(|| format!("missing remote {name}"))
}
fn now() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs())
}
fn key(value: &str) -> Result<(), String> {
    storage::validate_run_id(value)
        .map_err(|_| "request ID must be 1..128 ASCII letters, digits, - or _".into())
}
fn absolute(value: &str) -> Result<(), String> {
    if !value.starts_with('/')
        || value.contains(['\0', '\n', '\r'])
        || value.split('/').any(|p| matches!(p, "." | ".."))
    {
        return Err("remote paths must be explicit absolute canonical Linux paths without . or .. components".into());
    }
    Ok(())
}
fn host(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.starts_with('-')
        || value.len() > 255
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-@:[]".contains(&b))
    {
        return Err(
            "invalid SSH host; use an alias or [user@]hostname, never shell syntax/options".into(),
        );
    }
    Ok(())
}
fn username(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || value.starts_with('-')
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err("invalid explicit remote user".into());
    }
    Ok(())
}
fn endpoint_id(value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("endpoint-id must be a separately provisioned 64-hex public nonce".into());
    }
    Ok(())
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn random() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "cannot obtain remote request randomness")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn read(path: &Path) -> Result<Value, String> {
    lifecycle::read_json(path)
}
pub(super) fn sync_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("sync remote directory: {e}"))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn ancestors(path: &Path) -> Result<(), String> {
    let physical = path
        .canonicalize()
        .map_err(|e| format!("resolve private path: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for parent in physical.ancestors() {
            let metadata = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
            if metadata.file_type().is_symlink()
                || (metadata.uid() != unsafe { libc::geteuid() } && metadata.uid() != 0)
                || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
            {
                return Err("unsafe private state ancestor".into());
            }
        }
    }
    #[cfg(not(unix))]
    let _ = physical;
    Ok(())
}
fn existing(path: &Path, directory: bool) -> Result<(), String> {
    lifecycle::private_path(path, directory)?;
    ancestors(path)
}
fn trusted_public(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("public trust state must be a regular non-symlink file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (metadata.uid() != unsafe { libc::geteuid() } && metadata.uid() != 0)
            || metadata.mode() & 0o022 != 0
        {
            return Err(
                "public trust state must be trusted-owner and not group/other-writable".into(),
            );
        }
    }
    ancestors(path)
}
fn public_record(path: &Path) -> Result<Value, String> {
    trusted_public(path)?;
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| f.take(65537).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("oversized public endpoint descriptor".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid public endpoint descriptor".into())
}
fn directory(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("missing private directory parent")?;
    ancestors(parent)?;
    match crate::private_dir(path) {
        Ok(()) => sync_dir(parent)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("create private remote directory: {e}")),
    }
    existing(path, true)
}
fn write_new(path: &Path, value: &Value) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("create remote record: {e}"))?;
    serde_json::to_writer(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n")
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    sync_dir(path.parent().ok_or("missing record parent")?)
}
fn lock(path: &Path) -> Result<File, String> {
    if fs::symlink_metadata(path).is_ok() {
        existing(path, false)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|_| "remote request has a live owner; retry reconciliation")?;
    Ok(file)
}
fn strings(value: &Value) -> Result<Vec<String>, String> {
    let array = value
        .as_array()
        .filter(|v| v.len() <= 256)
        .ok_or("remote args must be an array of at most 256 strings")?;
    array
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "remote arguments must be strings".into())
        })
        .collect()
}
fn execute_policy(context: &Value, issuer: &str) -> Result<(), String> {
    if text(context, "caller_thread_id")? != issuer {
        return Err("signed caller identity mismatch".into());
    }
    if profile::Execution::from_context(context)?.mode != "danger-full-access" {
        return Err("remote v2 requires fresh full-access/approval-never for execution/steer; restricted filesystem policies cannot be translated across hosts".into());
    }
    Ok(())
}
fn verified_root(args: &[String]) -> Result<Value, String> {
    crate::subteam::authorize(args)?;
    if crate::subteam::caller_run()?.is_some() {
        return Err("only a verified root main may choose remote host or manage authority keys; coordinator/ordinary worker remote hops are forbidden".into());
    }
    let context = profile::load()?;
    profile::validate_session_id(text(&context, "caller_thread_id")?)?;
    Ok(context)
}
fn appended(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}
fn public_key(key: &Path) -> Result<String, String> {
    let output = Command::new("ssh-keygen")
        .args(["-y", "-P", "", "-f"])
        .arg(key)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "OpenSSH ssh-keygen is required")?;
    if !output.status.success() {
        return Err(
            "authority key must be a readable launcher-owned unencrypted OpenSSH key".into(),
        );
    }
    let output = String::from_utf8(output.stdout).map_err(|_| "invalid public authority key")?;
    let fields = output.split_whitespace().take(2).collect::<Vec<_>>();
    if fields.len() != 2 || fields[0] != "ssh-ed25519" {
        return Err("authority requires an Ed25519 key".into());
    }
    Ok(fields.join(" "))
}
fn keygen(args: &[String]) -> Result<(), String> {
    let context = verified_root(args)?;
    let issuer = text(&context, "caller_thread_id")?;
    execute_policy(&context, issuer)?;
    if args.len() != 3 || args[1] != "--authority-key" {
        return Err("expected remote-keygen --authority-key LOCAL_ABSOLUTE_PATH".into());
    }
    let path = PathBuf::from(&args[2]);
    if !path.is_absolute() {
        return Err("authority key path must be local and absolute".into());
    }
    ancestors(path.parent().ok_or("missing authority key parent")?)?;
    for candidate in [
        path.clone(),
        appended(&path, ".pub"),
        appended(&path, ".owner.json"),
    ] {
        if fs::symlink_metadata(candidate).is_ok() {
            return Err("authority paths already exist; refusing overwrite".into());
        }
    }
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", issuer, "-f"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| "OpenSSH ssh-keygen is required")?;
    if !status.success() {
        return Err("authority key generation failed; inspect/remove only owned partial key files before retrying".into());
    }
    existing(&path, false)?;
    File::open(&path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    let public = public_key(&path)?;
    write_new(
        &appended(&path, ".owner.json"),
        &json!({"version":VERSION,"issuer":issuer,"public":public}),
    )?;
    println!(
        "{}",
        json!({"issuer":issuer,"public_key":appended(&path,".pub"),
        "allowed_signers_line":format!("{issuer} namespaces=\"{NAMESPACE}\" {public}"),
        "enrollment":"operator must provision public line separately at <actual-remote-executable>.allowed-signers; never copy the private key"})
    );
    Ok(())
}
fn sign(grant: &Value, authority: &Path) -> Result<Value, String> {
    existing(authority, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(authority).map_err(|e| e.to_string())?.mode() & 0o077 != 0 {
            return Err("authority private key must not be accessible by group/others".into());
        }
    }
    let retained = read(&appended(authority, ".owner.json"))?;
    if retained["version"] != VERSION
        || retained["issuer"] != grant["issuer"]
        || retained["public"] != public_key(authority)?
    {
        return Err("authority key is not bound to this verified root; use remote-keygen and separate public enrollment".into());
    }
    sign_with(grant, authority)
}
fn sign_with(grant: &Value, authority: &Path) -> Result<Value, String> {
    let data = serde_json::to_vec(grant).map_err(|e| e.to_string())?;
    if data.len() as u64 > LIMIT - 16384 {
        return Err("remote grant exceeds protocol limit".into());
    }
    let mut child = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-f"])
        .arg(authority)
        .args(["-n", NAMESPACE])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "OpenSSH ssh-keygen with -Y signing support is required")?;
    let sent = child
        .stdin
        .take()
        .ok_or("signer stdin unavailable")?
        .write_all(&data);
    let output = child
        .wait_with_output()
        .map_err(|_| "authority signing failed")?;
    if sent.is_err() || !output.status.success() || output.stdout.len() > 8192 {
        return Err("authority signing failed; protected unencrypted Ed25519 key required".into());
    }
    Ok(
        json!({"version":VERSION,"grant":grant,"signature":String::from_utf8(output.stdout).map_err(|_| "invalid authority signature")?}),
    )
}

struct SignatureFile(PathBuf);
impl Drop for SignatureFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("signature"));
        let _ = fs::remove_file(self.0.join("allowed-signers"));
        let _ = fs::remove_dir(&self.0);
    }
}
fn verify(envelope: &Value, anchor: &Path) -> Result<Value, String> {
    trusted_public(anchor).map_err(|_| "missing/unsafe fixed endpoint allowed-signers anchor; operator must separately enroll public authority")?;
    let mut anchor_bytes = Vec::new();
    File::open(anchor)
        .and_then(|f| f.take(65537).read_to_end(&mut anchor_bytes))
        .map_err(|_| "cannot read public authority anchor")?;
    verify_with_keys(envelope, &anchor_bytes)
}
fn verify_with_keys(envelope: &Value, anchor_bytes: &[u8]) -> Result<Value, String> {
    if envelope["version"] != VERSION || envelope.as_object().is_none_or(|o| o.len() != 3) {
        return Err("remote protocol version/schema mismatch; require signed v2".into());
    }
    let grant = &envelope["grant"];
    let signature = text(envelope, "signature")?;
    if signature.len() > 8192 {
        return Err("oversized authority signature".into());
    }
    let issuer = text(grant, "issuer")?;
    profile::validate_session_id(issuer)?;
    // This candidate is enrolled to one exact root principal (key rotation may
    // provide several entries for that same principal). Wildcards/CSV aliases
    // must not turn a valid key into authority to assert arbitrary root IDs.
    if anchor_bytes.len() > 65536 {
        return Err("oversized public authority anchor".into());
    }
    let anchor_text =
        std::str::from_utf8(&anchor_bytes).map_err(|_| "invalid public authority anchor")?;
    let principals = anchor_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split_whitespace().next().unwrap())
        .collect::<Vec<_>>();
    if principals.is_empty() || principals.iter().any(|principal| *principal != issuer) {
        return Err("endpoint trust anchor must enroll this exact root principal only; wildcard/self-enrollment is forbidden".into());
    }
    let dir = std::env::temp_dir().join(format!("pira-team-signature-{}", random()?));
    crate::private_dir(&dir).map_err(|e| e.to_string())?;
    let temp = SignatureFile(dir);
    let anchor = temp.0.join("allowed-signers");
    crate::create(&anchor)?
        .write_all(anchor_bytes)
        .map_err(|e| e.to_string())?;
    let mut file = crate::create(&temp.0.join("signature"))?;
    file.write_all(signature.as_bytes())
        .map_err(|e| e.to_string())?;
    drop(file);
    let mut child = Command::new("ssh-keygen")
        .args(["-Y", "verify", "-f"])
        .arg(&anchor)
        .args(["-I", issuer, "-n", NAMESPACE, "-s"])
        .arg(temp.0.join("signature"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "OpenSSH ssh-keygen with -Y verification support is required")?;
    let data = serde_json::to_vec(grant).map_err(|e| e.to_string())?;
    if data.len() as u64 > LIMIT {
        let _ = child.kill();
        let _ = child.wait();
        return Err("oversized signed grant".into());
    }
    let sent = child
        .stdin
        .take()
        .ok_or("verifier stdin unavailable")?
        .write_all(&data);
    let status = child.wait().map_err(|_| "authority verifier failed")?;
    if sent.is_err() || !status.success() {
        return Err(
            "remote launcher signature is not authorized by the fixed endpoint trust anchor".into(),
        );
    }
    validate_grant(grant)?;
    Ok(grant.clone())
}
fn validate_grant(grant: &Value) -> Result<(), String> {
    const FIELDS: &[&str] = &[
        "protocol",
        "scope",
        "issuer",
        "endpoint",
        "op",
        "request_id",
        "run_request",
        "args",
        "context",
        "issued_at",
        "expires_at",
        "nonce",
        "file",
        "artifact",
        "operation_id",
        "cwd",
        "target",
    ];
    if grant["protocol"] != VERSION
        || grant["scope"] != SCOPE
        || grant.as_object().is_none_or(|o| {
            o.len() != FIELDS.len() || o.keys().any(|k| !FIELDS.contains(&k.as_str()))
        })
    {
        return Err("unsupported signed grant protocol/scope/schema".into());
    }
    key(text(grant, "request_id")?)?;
    profile::validate_session_id(text(grant, "issuer")?)?;
    if !text(grant, "nonce")?.bytes().all(|b| b.is_ascii_hexdigit())
        || text(grant, "nonce")?.len() != 64
    {
        return Err("invalid signed grant nonce".into());
    }
    let issued = grant["issued_at"]
        .as_u64()
        .ok_or("missing grant issue time")?;
    let expires = grant["expires_at"].as_u64().ok_or("missing grant expiry")?;
    if expires <= issued || expires - issued > VALIDITY {
        return Err("invalid signed grant validity interval".into());
    }
    let endpoint = &grant["endpoint"];
    if endpoint.as_object().is_none_or(|o| o.len() != 6) {
        return Err("invalid signed endpoint schema".into());
    }
    host(text(endpoint, "host")?)?;
    username(text(endpoint, "user")?)?;
    endpoint_id(text(endpoint, "endpoint_id")?)?;
    for name in ["exe", "backend", "store"] {
        absolute(text(endpoint, name)?)?;
    }
    let op = text(grant, "op")?;
    let args = strings(&grant["args"])?;
    if !matches!(op, "submit" | "read" | "steer" | "interrupt" | "probe") {
        return Err("unsupported signed operation".into());
    }
    if !grant["artifact"].is_boolean() || (!grant["file"].is_null() && !grant["file"].is_string()) {
        return Err("invalid remote read selection".into());
    }
    if grant["artifact"] == true && !grant["file"].is_null()
        || op == "read"
            && !grant["operation_id"].is_null()
            && (grant["artifact"] == true || !grant["file"].is_null())
    {
        return Err("ambiguous remote read selector".into());
    }
    if op == "read"
        && args.first().map(String::as_str) == Some("capabilities")
        && (grant["artifact"] == true
            || !grant["file"].is_null()
            || !grant["operation_id"].is_null())
    {
        return Err("capability metadata read cannot select run artifacts/controls".into());
    }
    if op != "read" && (grant["artifact"] == true || !grant["file"].is_null()) {
        return Err("read selection on non-read operation".into());
    }
    if matches!(op, "submit" | "steer" | "probe") {
        execute_policy(&grant["context"], text(grant, "issuer")?)?;
    } else if !grant["context"].is_null() {
        return Err("observation/control grants must not assert an execution context".into());
    }
    if op == "submit" {
        match args.first().map(String::as_str) {
            Some("run") => {
                absolute(text(grant, "cwd")?)?;
                if !grant["run_request"].is_null() {
                    return Err("run cannot target a prior request".into());
                }
            }
            Some("resume") => {
                key(text(grant, "run_request")?)?;
                if !grant["cwd"].is_null() {
                    return Err("resume uses retained remote cwd".into());
                }
            }
            _ => return Err("submit requires run/resume argv".into()),
        }
        validate_args(&args)?;
    } else {
        if !grant["cwd"].is_null() || !grant["run_request"].is_null() {
            return Err("unexpected path/run override on control/read".into());
        }
        match op {
            "steer" => {
                key(text(grant, "operation_id")?)?;
                control_task(&args)?;
            }
            "interrupt" => {
                key(text(grant, "operation_id")?)?;
                if args.first().map(String::as_str) != Some("interrupt")
                    || args
                        .iter()
                        .skip(1)
                        .any(|a| !matches!(a.as_str(), "-r" | "--recursive"))
                {
                    return Err("invalid interrupt argv".into());
                }
            }
            "read" | "probe" => {
                if args.len() != 1 {
                    return Err("read/probe take no positional arguments".into());
                }
            }
            _ => {}
        }
    }
    if !matches!(op, "steer" | "interrupt" | "read") && !grant["operation_id"].is_null() {
        return Err("unexpected operation-id".into());
    }
    if op == "read" && !grant["operation_id"].is_null() {
        key(text(grant, "operation_id")?)?;
    }
    if matches!(op, "steer" | "interrupt") {
        validate_target(&grant["target"])?;
    } else if !grant["target"].is_null() {
        return Err("unexpected control target".into());
    }
    Ok(())
}
fn fresh(grant: &Value, at: u64) -> Result<(), String> {
    let issued = grant["issued_at"]
        .as_u64()
        .ok_or("missing grant issue time")?;
    let expires = grant["expires_at"].as_u64().ok_or("missing grant expiry")?;
    if issued > at.saturating_add(30) || at > expires {
        return Err("signed grant is expired/not yet valid; accepted requests may reconcile, never replay unknown delivery".into());
    }
    Ok(())
}

struct Client {
    endpoint: Value,
    authority: PathBuf,
    state: PathBuf,
    id: String,
    run_request: Option<String>,
    operation: Option<String>,
    cwd: Option<String>,
    detach: bool,
    artifact: bool,
    file: Option<String>,
    args: Vec<String>,
}
impl Client {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut fields = std::collections::BTreeMap::new();
        let mut forwarded = vec![args[0].clone()];
        let (mut detach, mut artifact) = (false, false);
        let mut rest = args.iter().skip(1);
        while let Some(arg) = rest.next() {
            if arg == "--detach" {
                if detach {
                    return Err("duplicate --detach".into());
                }
                detach = true;
                continue;
            }
            if arg == "--artifact" {
                if artifact {
                    return Err("duplicate --artifact".into());
                }
                artifact = true;
                continue;
            }
            let name = arg.split('=').next().unwrap();
            if ["--cwd", "--store", "--schema", "--label"].contains(&name) {
                return Err(format!(
                    "{name} is unsupported remotely; local paths never grant remote filesystem authority"
                ));
            }
            if [
                "--host",
                "--remote-user",
                "--remote-endpoint-id",
                "--remote-exe",
                "--remote-backend",
                "--remote-store",
                "--remote-state",
                "--authority-key",
                "--remote-cwd",
                "--request-id",
                "--run-request",
                "--operation-id",
                "--file",
            ]
            .contains(&name)
            {
                let value = if let Some((_, value)) = arg.split_once('=') {
                    value.to_owned()
                } else {
                    rest.next()
                        .ok_or_else(|| format!("missing {name}"))?
                        .clone()
                };
                if fields.insert(name.to_owned(), value).is_some() {
                    return Err(format!("duplicate {name}"));
                }
            } else if arg == "--task-file" {
                let path = rest.next().ok_or("missing --task-file")?;
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
            } else if arg == "--" {
                forwarded.push(arg.clone());
                forwarded.extend(rest.cloned());
                break;
            } else {
                forwarded.push(arg.clone());
                if arg.starts_with('-') && !arg.contains('=') && !flag(arg) {
                    forwarded.push(rest.next().ok_or_else(|| format!("missing {arg}"))?.clone());
                }
            }
        }
        let mut take = |name: &str| {
            fields
                .remove(name)
                .ok_or_else(|| format!("remote mode requires {name}"))
        };
        let host_name = take("--host")?;
        host(&host_name)?;
        let user = take("--remote-user")?;
        username(&user)?;
        let identity = take("--remote-endpoint-id")?;
        endpoint_id(&identity)?;
        if let Some((selected, _)) = host_name.split_once('@') {
            if selected != user {
                return Err("host user and --remote-user must agree".into());
            }
        }
        let exe = take("--remote-exe")?;
        absolute(&exe)?;
        let backend = take("--remote-backend")?;
        absolute(&backend)?;
        let store = take("--remote-store")?;
        absolute(&store)?;
        let state = PathBuf::from(take("--remote-state")?);
        let authority = PathBuf::from(take("--authority-key")?);
        if !state.is_absolute() || !authority.is_absolute() {
            return Err(
                "--remote-state and --authority-key are explicit local absolute paths".into(),
            );
        }
        let id = take("--request-id")?;
        key(&id)?;
        let run_request = fields.remove("--run-request");
        if let Some(id) = &run_request {
            key(id)?;
        }
        let operation = fields.remove("--operation-id");
        if let Some(id) = &operation {
            key(id)?;
        }
        let cwd = fields.remove("--remote-cwd");
        if let Some(cwd) = &cwd {
            absolute(cwd)?;
        }
        let file = fields.remove("--file");
        let op = args[0].as_str();
        if ![
            "run",
            "resume",
            "steer",
            "interrupt",
            "attach",
            "read",
            "remote-check",
        ]
        .contains(&op)
        {
            return Err("unsupported remote command".into());
        }
        if (op == "run") != cwd.is_some() {
            return Err("--remote-cwd is required only for remote run".into());
        }
        if (op == "resume") != run_request.is_some() {
            return Err(
                "resume requires --run-request ORIGINAL_REQUEST; unsupported for other commands"
                    .into(),
            );
        }
        if ["steer", "interrupt"].contains(&op) && operation.is_none() {
            return Err(
                "steer/interrupt require --operation-id for durable delivery reconciliation".into(),
            );
        }
        if operation.is_some() && !["steer", "interrupt", "read"].contains(&op) {
            return Err("operation-id is only for controls or reading a control receipt".into());
        }
        if detach && !["run", "resume"].contains(&op) {
            return Err("--detach is only for run/resume".into());
        }
        if (artifact || file.is_some()) && op != "read" {
            return Err("--artifact/--file is only for read".into());
        }
        if usize::from(artifact)
            + usize::from(file.is_some())
            + usize::from(operation.is_some() && op == "read")
            > 1
        {
            return Err("choose artifact, file, or operation receipt".into());
        }
        if ["read", "attach", "remote-check"].contains(&op) && forwarded.len() != 1 {
            return Err("remote reads/attach/check take options, not positional run IDs".into());
        }
        Ok(Self {
            endpoint: json!({"host":host_name,"user":user,"endpoint_id":identity,"exe":exe,"backend":backend,"store":store}),
            authority,
            state,
            id,
            run_request,
            operation,
            cwd,
            detach,
            artifact,
            file,
            args: forwarded,
        })
    }
    fn grant(&self, op: &str, issuer: &str, context: Option<&Value>) -> Result<Value, String> {
        let time = now()?;
        Ok(
            json!({"protocol":VERSION,"scope":SCOPE,"issuer":issuer,"endpoint":self.endpoint,"op":op,
            "request_id":self.id,"run_request":self.run_request,"args":self.args,"context":context,
            "issued_at":time,"expires_at":time+VALIDITY,"nonce":random()?,"file":self.file,
            "artifact":self.artifact,"operation_id":self.operation,"cwd":self.cwd,"target":null}),
        )
    }
    fn exchange(&self, envelope: &Value) -> Result<Value, String> {
        let mut ssh = Command::new("ssh");
        ssh.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "ControlPersist=no",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "AddKeysToAgent=no",
            "-o",
            "ForwardX11=no",
            "-o",
            "CheckHostIP=no",
            "-o",
            "RemoteCommand=none",
            "-l",
            text(&self.endpoint, "user")?,
            "--",
            text(&self.endpoint, "host")?,
        ]);
        ssh.arg(format!(
            "exec {} __remote-serve",
            quote(text(&self.endpoint, "exe")?)
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
        let mut child = ssh.spawn().map_err(|e| format!("start SSH: {e}"))?;
        let sent = (|| {
            let mut input = child.stdin.take().ok_or("SSH stdin unavailable")?;
            serde_json::to_writer(&mut input, envelope).map_err(|e| e.to_string())?;
            input.write_all(b"\n").map_err(|e| e.to_string())
        })();
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or("SSH stdout unavailable")?
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("read SSH: {e}"))?;
        if bytes.len() as u64 > LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("remote reply exceeds protocol limit".into());
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        if sent.is_err() || !status.success() {
            return Err("SSH delivery/connectivity unknown; reconcile SAME request/operation ID, never replay with a new ID; worker was not stopped".into());
        }
        let reply: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "invalid remote response/protocol mismatch; delivery unknown")?;
        if reply["version"] != VERSION {
            return Err("remote protocol version mismatch; signed v2 required".into());
        }
        if reply["ok"] != true {
            return Err(reply["error"]
                .as_str()
                .unwrap_or("remote request failed")
                .to_owned());
        }
        Ok(reply["result"].clone())
    }
    fn capability_request(&self, issuer: &str) -> Result<Value, String> {
        let mut grant = self.grant("read", issuer, None)?;
        grant["args"] = json!(["capabilities"]);
        grant["cwd"] = Value::Null;
        grant["run_request"] = Value::Null;
        grant["operation_id"] = Value::Null;
        grant["file"] = Value::Null;
        grant["artifact"] = json!(false);
        validate_grant(&grant)?;
        Ok(grant)
    }
    fn metadata(&self, issuer: &str) -> Result<Value, String> {
        let grant = self.capability_request(issuer)?;
        let metadata = self.exchange(&sign(&grant, &self.authority)?)?;
        require_capabilities(&metadata, "read", false)?;
        Ok(metadata)
    }
    fn negotiate(&self, issuer: &str, operation: &str, subteam: bool) -> Result<(), String> {
        require_capabilities(&self.metadata(issuer)?, operation, subteam)
    }
    fn operation_receipt(&self, issuer: &str) -> Result<Value, String> {
        let mut grant = self.grant("read", issuer, None)?;
        grant["args"] = json!(["read"]);
        grant["cwd"] = Value::Null;
        grant["run_request"] = Value::Null;
        grant["file"] = Value::Null;
        grant["artifact"] = json!(false);
        self.exchange(&sign(&grant, &self.authority)?)
    }
    fn observe(&self, issuer: &str) -> Result<Value, String> {
        self.observe_request(issuer, &self.id)
    }
    fn observe_request(&self, issuer: &str, request: &str) -> Result<Value, String> {
        let mut grant = self.grant("read", issuer, None)?;
        grant["request_id"] = json!(request);
        grant["args"] = json!(["read"]);
        grant["cwd"] = Value::Null;
        grant["run_request"] = Value::Null;
        grant["operation_id"] = Value::Null;
        grant["file"] = Value::Null;
        grant["artifact"] = json!(false);
        self.exchange(&sign(&grant, &self.authority)?)
    }
}
fn logical(grant: &Value) -> Value {
    let mut result = grant.clone();
    if let Some(fields) = result.as_object_mut() {
        for field in ["issued_at", "expires_at", "nonce"] {
            fields.remove(field);
        }
    }
    result
}
fn same_owner(original: &Value, grant: &Value) -> Result<(), String> {
    if original["issuer"] != grant["issuer"] || original["endpoint"] != grant["endpoint"] {
        return Err("remote run unavailable to this verified root/endpoint".into());
    }
    Ok(())
}
// Only one immutable request file is written. Other IDs share no mutable
// client index; server lifetime ownership remains a separate nonblocking lock.
fn persist_submission(
    state: &Path,
    grant: &Value,
    signer: impl FnOnce(&Value) -> Result<Value, String>,
) -> Result<(Value, Option<Value>), String> {
    validate_grant(grant)?;
    directory(state)?;
    let id = text(grant, "request_id")?;
    let _lock = lock(&state.join(format!("request-{id}.lock")))?;
    let origin = if grant["args"][0] == "resume" {
        let original = read(&state.join(format!("{}.json", text(grant, "run_request")?)))?;
        same_owner(&original["grant"], grant)?;
        if original["grant"]["args"][0] != "run" {
            return Err("--run-request must select the original run submission".into());
        }
        Some(original["grant"].clone())
    } else {
        None
    };
    let path = state.join(format!("{id}.json"));
    let envelope = if fs::symlink_metadata(&path).is_ok() {
        let retained = read(&path)?;
        if logical(&retained["grant"]) != logical(grant) {
            return Err("request ID already binds different payload/caller/policy/endpoint".into());
        }
        retained
    } else {
        let envelope = signer(grant)?;
        write_new(&path, &envelope)?;
        envelope
    };
    Ok((envelope, origin))
}
fn client(args: &[String]) -> Result<(), String> {
    let context = verified_root(args)?;
    let cli = Client::parse(args)?;
    let issuer = text(&context, "caller_thread_id")?;
    let op = args[0].as_str();
    let execute = ["run", "resume", "steer", "remote-check"].contains(&op);
    if execute {
        execute_policy(&context, issuer)?;
    }
    // Never transmit local cwd/tool stores, sessions, or backend authentication.
    let execution = execute.then(|| json!({"caller_thread_id":issuer,"model":context["model"],"effort":context["effort"],
        "approval_policy":context["approval_policy"],"sandbox_policy":context["sandbox_policy"],
        "permission_profile":context["permission_profile"],"file_system_sandbox_policy":context["file_system_sandbox_policy"]}));
    let protocol_op = match op {
        "run" | "resume" => "submit",
        "attach" => "read",
        "remote-check" => "probe",
        value => value,
    };
    let mut grant = cli.grant(protocol_op, issuer, execution.as_ref())?;
    let subteam = requests_subteam(&cli.args);
    let mut resume_origin = None;
    if op == "remote-check" {
        cli.negotiate(issuer, op, false)?;
        println!("{}", cli.exchange(&sign(&grant, &cli.authority)?)?);
        return Ok(());
    }
    let envelope = if matches!(op, "run" | "resume") {
        let (envelope, origin) =
            persist_submission(&cli.state, &grant, |grant| sign(grant, &cli.authority))?;
        resume_origin = origin;
        envelope
    } else {
        // Observation and interruption after policy tightening do not create/lock/write local state.
        existing(&cli.state, true)?;
        let original = read(&cli.state.join(format!("{}.json", cli.id)))?;
        same_owner(&original["grant"], &grant)?;
        cli.negotiate(issuer, op, false)?;
        if matches!(op, "steer" | "interrupt") {
            let receipt = cli.operation_receipt(issuer)?;
            if receipt["status"] != "not_found" {
                let mut intended = logical(&grant);
                intended.as_object_mut().unwrap().remove("target");
                if receipt["intent"] != intended {
                    return Err("operation ID already binds different control intent; inspect its retained receipt".into());
                }
                println!("{receipt}");
                return Ok(());
            }
            grant["target"] = cli.observe(issuer)?["control_target"].clone();
            if grant["target"].is_null() {
                return Err("remote run has no retained control target; inspect request history before control".into());
            }
        }
        validate_grant(&grant)?;
        sign(&grant, &cli.authority)?
    };
    if op == "resume" {
        let metadata = cli.metadata(issuer)?;
        let retained = cli.observe_request(issuer, cli.run_request.as_deref().unwrap())?;
        require_resume_capabilities(
            &metadata,
            &retained,
            resume_origin.as_ref().unwrap(),
            &cli.args,
        )?;
    } else if op == "run" {
        routing::retain_transport(&cli, &envelope)?;
        cli.negotiate(issuer, op, subteam)?;
    }
    let mut result = cli.exchange(&envelope)?;
    if !matches!(op, "run" | "resume" | "attach") || cli.detach {
        println!("{result}");
        return Ok(());
    }
    loop {
        if crate::CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("attach disconnected; remote worker was not interrupted; attach again with SAME request ID".into());
        }
        if !result["operation_result"].is_null() {
            println!("{result}");
            return if result["operation_result"]["ok"] == true {
                Ok(())
            } else {
                Err("remote operation failed; inspect retained result, errors, and usage".into())
            };
        }
        if result["supervisor"] == "unknown" {
            println!("{result}");
            return Err("supervisor absent/startup uncertain; no automatic replay; inspect history and accounting".into());
        }
        std::thread::sleep(Duration::from_secs(1));
        result = cli.observe(issuer)?;
    }
}

fn validate_args(args: &[String]) -> Result<(), String> {
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') {
            continue;
        }
        let name = arg.split('=').next().unwrap();
        if [
            "--cwd",
            "--store",
            "--schema",
            "--label",
            "--task-file",
            "--host",
            "--remote-exe",
            "--remote-store",
        ]
        .contains(&name)
        {
            return Err(format!(
                "remote argv may not override signed endpoint/path selection with {name}"
            ));
        }
        if !arg.contains('=') && !flag(arg) {
            rest.next().ok_or("missing remote option value")?;
        }
    }
    Ok(())
}
fn invocation(grant: &Value, run: Option<&Path>) -> Result<Vec<String>, String> {
    let args = strings(&grant["args"])?;
    validate_args(&args)?;
    let mut result = vec![args.first().ok_or("missing signed argv")?.clone()];
    match result[0].as_str() {
        "run" => result.extend(["--cwd".into(), text(grant, "cwd")?.into()]),
        "resume" => result.push(
            run.ok_or("resume lacks retained run")?
                .file_name()
                .ok_or("invalid retained run")?
                .to_str()
                .ok_or("non UTF-8 run ID")?
                .into(),
        ),
        _ => return Err("invalid submitted argv operation".into()),
    }
    // Inject authoritative paths BEFORE argv's -- terminator, never after task data.
    result.extend(["--store".into(), text(&grant["endpoint"], "store")?.into()]);
    result.extend(args.into_iter().skip(1));
    Ok(result)
}
fn control_task(args: &[String]) -> Result<(String, String), String> {
    if args.first().map(String::as_str) != Some("steer") {
        return Err("control argv mismatch".into());
    }
    let (mut task, mut gate) = (None, None);
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        if arg == "--" {
            crate::set_once(
                &mut task,
                rest.next().ok_or("missing task after --")?.clone(),
                "task",
            )?;
            if rest.next().is_some() {
                return Err("provide exactly one task".into());
            }
        } else if !arg.starts_with('-') {
            crate::set_once(&mut task, arg.clone(), "task")?;
        } else if let Some(value) = arg.strip_prefix("--task=") {
            crate::set_once(&mut task, value.to_owned(), "task")?;
        } else if let Some(value) = arg.strip_prefix("--completion-gate=") {
            crate::set_once(&mut gate, value.to_owned(), "gate")?;
        } else if arg == "--task" {
            crate::set_once(
                &mut task,
                rest.next().ok_or("missing task")?.clone(),
                "task",
            )?;
        } else if arg == "--completion-gate" {
            crate::set_once(
                &mut gate,
                rest.next().ok_or("missing gate")?.clone(),
                "gate",
            )?;
        } else {
            return Err("steer requires --task and --completion-gate".into());
        }
    }
    Ok((
        crate::task_input(task, None, None)?,
        crate::completion_gate(gate)?,
    ))
}
fn canonical_path(path: &Path, present: bool) -> Result<(), String> {
    let physical = if present || path.exists() {
        path.canonicalize().map_err(|e| e.to_string())?
    } else {
        path.parent()
            .ok_or("missing remote parent")?
            .canonicalize()
            .map_err(|e| e.to_string())?
            .join(path.file_name().ok_or("missing remote path name")?)
    };
    if physical != path {
        return Err("remote paths must be canonical, without symlink/wrapper aliases".into());
    }
    Ok(())
}
fn executable(path: &Path) -> Result<(), String> {
    canonical_path(path, true)?;
    ancestors(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("remote executable/backend must be a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o111 == 0
            || metadata.mode() & 0o022 != 0
            || (metadata.uid() != unsafe { libc::geteuid() } && metadata.uid() != 0)
        {
            return Err("remote executable/backend must be trusted-owner executable and not group/other-writable".into());
        }
    }
    Ok(())
}
fn current_user() -> Result<String, String> {
    #[cfg(unix)]
    {
        // getpwuid result is copied immediately, before spawning any threads.
        let passwd = unsafe { libc::getpwuid(libc::geteuid()) };
        if passwd.is_null() {
            return Err("cannot resolve actual SSH account UID".into());
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*passwd).pw_name) }
            .to_str()
            .map_err(|_| "remote username requires UTF-8")?
            .to_owned();
        return Ok(name);
    }
    #[cfg(not(unix))]
    Err("remote endpoint requires Linux".into())
}
fn endpoint_identity() -> Result<(PathBuf, PathBuf, String), String> {
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| e.to_string())?;
    executable(&exe)?;
    Ok((
        exe.clone(),
        appended(&exe, ".allowed-signers"),
        current_user()?,
    ))
}
fn endpoint_binding(grant: &Value, exe: &Path, user: &str) -> Result<(), String> {
    let endpoint = &grant["endpoint"];
    let descriptor_path = appended(exe, ".remote-endpoint.json");
    trusted_public(&descriptor_path).map_err(
        |_| "missing/unsafe fixed endpoint descriptor; operator must provision it separately",
    )?;
    let descriptor = public_record(&descriptor_path)?;
    if descriptor.as_object().is_none_or(|o| o.len() != 4)
        || descriptor["version"] != VERSION
        || descriptor["endpoint_id"] != endpoint["endpoint_id"]
        || descriptor["user"] != user
        || descriptor["exe"] != json!(exe)
    {
        return Err(
            "signed endpoint does not match independently provisioned endpoint identity".into(),
        );
    }
    if Path::new(text(endpoint, "exe")?) != exe || text(endpoint, "user")? != user {
        return Err(
            "signed endpoint differs from actual executable/SSH user; wrappers are unsupported"
                .into(),
        );
    }
    if matches!(text(grant, "op")?, "submit" | "steer" | "probe") {
        executable(Path::new(text(endpoint, "backend")?))?;
    }
    let store = Path::new(text(endpoint, "store")?);
    canonical_path(store, false)?;
    ancestors(if store.exists() {
        store
    } else {
        store.parent().ok_or("missing store parent")?
    })?;
    if grant["op"] == "submit" && grant["args"][0] == "run" {
        let cwd = Path::new(text(grant, "cwd")?);
        canonical_path(cwd, true)?;
        if !cwd.is_dir() {
            return Err("remote cwd must be an existing directory".into());
        }
    }
    Ok(())
}
fn input() -> Result<Value, String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("remote request exceeds 16 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid remote protocol JSON".into())
}
fn serve() -> Result<(), String> {
    let result = input().and_then(endpoint);
    let reply = match result {
        Ok(result) => json!({"version":VERSION,"ok":true,"result":result}),
        Err(error) => json!({"version":VERSION,"ok":false,"error":error}),
    };
    println!("{reply}");
    Ok(())
}
fn endpoint(envelope: Value) -> Result<Value, String> {
    if !cfg!(target_os = "linux") {
        return Err("remote supervisor v2 requires Linux; local commands remain supported".into());
    }
    let (exe, anchor, user) = endpoint_identity()?;
    let grant = verify(&envelope, &anchor)?;
    endpoint_binding(&grant, &exe, &user)?;
    // No generic ancestry walk through privileged SSH monitors. Only this verified,
    // bounded grant can install process-local root context; children start unbound.
    match text(&grant, "op")? {
        "probe" => {
            fresh(&grant, now()?)?;
            let available = Command::new(text(&grant["endpoint"], "backend")?)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            let mut metadata = capability_metadata();
            metadata["backend_available"] = json!(available);
            metadata["file_auth_present"] = json!(crate::authentication::caller_file()?.is_some());
            metadata["api_key_present"] =
                json!(std::env::var_os("CODEX_API_KEY").is_some_and(|v| !v.is_empty()));
            metadata["credential_validity"] =
                json!("not checked; launcher-managed authentication only");
            Ok(metadata)
        }
        "submit" => submit_using(envelope, &grant, &anchor, start_supervisor),
        "read" => {
            fresh(&grant, now()?)?;
            if grant["args"][0] == "capabilities" {
                return Ok(capability_metadata());
            }
            if unaccepted(&grant)? {
                return Ok(
                    json!({"request_id":grant["request_id"],"accepted":false,"status":"not_found"}),
                );
            }
            let (dir, record) = owned(&grant, &anchor)?;
            if grant["operation_id"].is_string() {
                control_view(&dir, text(&grant, "operation_id")?, &anchor)
            } else {
                view(&dir, &record, &grant, &anchor)
            }
        }
        "steer" | "interrupt" => {
            fresh(&grant, now()?)?;
            let (dir, record) = owned(&grant, &anchor)?;
            install(&grant)?;
            control_using(&dir, &record, envelope, &grant, &anchor, |run, target| {
                if grant["op"] == "steer" {
                    let (task, gate) = control_task(&strings(&grant["args"])?)?;
                    crate::app_server::control_bound(run, "steer", &task, Some(&gate), target)
                } else {
                    crate::subteam::interrupt_bound(
                        run,
                        grant["args"].as_array().unwrap().len() > 1,
                        target,
                    )
                }
            })
        }
        _ => Err("unsupported remote operation".into()),
    }
}
fn request_dir(grant: &Value) -> Result<PathBuf, String> {
    let store = Path::new(text(&grant["endpoint"], "store")?);
    canonical_path(store, true)?;
    existing(store, true)?;
    let ledger = store.join(".remote-v2");
    existing(&ledger, true)?;
    let id = text(grant, "request_id")?;
    key(id)?;
    Ok(ledger.join(id))
}
// Absence is distinct from a partial claim. Only absence permits reconciliation
// of the original submission; malformed/partial ledgers remain fail-closed.
fn unaccepted(grant: &Value) -> Result<bool, String> {
    let store = Path::new(text(&grant["endpoint"], "store")?);
    let ledger = store.join(".remote-v2");
    let request = ledger.join(text(grant, "request_id")?);
    for path in [store, ledger.as_path(), request.as_path()] {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(e) => return Err(e.to_string()),
            Ok(_) => existing(path, true)?,
        }
    }
    Ok(false)
}
fn record_run(record: &Value, anchor: &Path) -> Result<PathBuf, String> {
    let original = verify(&record["envelope"], anchor)?;
    if original["op"] != "submit" {
        return Err("retained ledger is not a signed submission".into());
    }
    let root_grant = if original["args"][0] == "resume" {
        let id = text(&original, "run_request")?;
        key(id)?;
        let root_record = read(
            &Path::new(text(&original["endpoint"], "store")?)
                .join(".remote-v2")
                .join(id)
                .join("request.json"),
        )?;
        let root_grant = verify(&root_record["envelope"], anchor)?;
        same_owner(&root_grant, &original)?;
        if root_grant["request_id"] != id
            || root_grant["op"] != "submit"
            || root_grant["args"][0] != "run"
        {
            return Err("resume must refer to original run request, not another resume".into());
        }
        root_grant
    } else {
        original.clone()
    };
    let expected = Path::new(text(&original["endpoint"], "store")?)
        .join(format!("remote-{}", text(&root_grant, "nonce")?));
    let run = PathBuf::from(text(record, "run")?);
    if run != expected {
        return Err("retained run escapes signed endpoint store".into());
    }
    key(run
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid retained run ID")?)?;
    canonical_path(&run, true)?;
    existing(&run, true)?;
    if run.join("manifest.json").exists() {
        let manifest = read(&run.join("manifest.json"))?;
        if manifest["creator_main_session_id"] != original["issuer"]
            || !manifest["parent_run_id"].is_null()
        {
            return Err(
                "retained run is not owned by signed root; grandchildren are not root-addressable"
                    .into(),
            );
        }
    }
    Ok(run)
}
fn owned(grant: &Value, anchor: &Path) -> Result<(PathBuf, Value), String> {
    let dir = request_dir(grant)?;
    existing(&dir, true)?;
    let record = read(&dir.join("request.json"))?;
    same_owner(&record["envelope"]["grant"], grant)?;
    if record["envelope"]["grant"]["request_id"] != grant["request_id"] {
        return Err("signed request ID differs from its ledger location".into());
    }
    record_run(&record, anchor)?;
    Ok((dir, record))
}
fn submit_using(
    envelope: Value,
    grant: &Value,
    anchor: &Path,
    spawn: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Value, String> {
    let store = PathBuf::from(text(&grant["endpoint"], "store")?);
    canonical_path(&store, false)?;
    if !store.exists() {
        fresh(grant, now()?)?;
        directory(&store)?;
    }
    existing(&store, true)?;
    let ledger = store.join(".remote-v2");
    if !ledger.exists() {
        fresh(grant, now()?)?;
        directory(&ledger)?;
    }
    existing(&ledger, true)?;
    let id = text(grant, "request_id")?;
    let dir = ledger.join(id);
    if fs::symlink_metadata(&dir).is_ok() {
        let (dir, record) = owned(grant, anchor)?;
        if logical(&record["envelope"]["grant"]) != logical(grant) {
            return Err("request ID already binds different signed payload".into());
        }
        return view(&dir, &record, &json!({}), anchor);
    }
    let accepted_at = now()?;
    fresh(grant, accepted_at)?;
    match crate::private_dir(&dir) {
        Ok(()) => sync_dir(&ledger)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(
                "request acceptance concurrent/uncertain; reconcile SAME request ID".into(),
            );
        }
        Err(e) => return Err(format!("claim remote request: {e}")),
    }
    // Persist the claim directory before any run creation. Partial state is a
    // fail-closed tombstone; neither endpoint retry nor supervisor restart replays.
    let run = if grant["args"][0] == "run" {
        let run = store.join(format!("remote-{}", text(grant, "nonce")?));
        crate::private_dir(&run).map_err(|e| format!("create signed run allocation: {e}"))?;
        run
    } else {
        let mut target = grant.clone();
        target["request_id"] = json!(text(grant, "run_request")?);
        let (_, original) = owned(&target, anchor)?;
        record_run(&original, anchor)?
    };
    sync_dir(&store)?;
    let record = json!({"envelope":envelope,"run":run,"accepted_at":accepted_at});
    write_new(&dir.join("request.json"), &record)?;
    write_new(&dir.join("claimed.json"), &json!({"claimed":true}))?;
    spawn(&dir)?;
    Ok(
        json!({"request_id":id,"run_id":run.file_name().unwrap().to_string_lossy(),"accepted":true,"supervisor":"starting","operation_result":null}),
    )
}
fn control_view(dir: &Path, id: &str, anchor: &Path) -> Result<Value, String> {
    key(id)?;
    let path = dir.join("controls").join(id);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"operation_id":id,"status":"not_found"}));
        }
        Err(e) => return Err(e.to_string()),
        Ok(_) => {}
    }
    existing(&path, true)?;
    // Missing/partial state never authorizes another effect.
    let retained = read(&path.join("request.json"))?;
    let original = verify(&retained, anchor)?;
    if original["operation_id"] != id
        || original["request_id"] != dir.file_name().unwrap().to_str().unwrap_or("")
        || !matches!(text(&original, "op")?, "steer" | "interrupt")
    {
        return Err("control ledger differs from its signed operation binding".into());
    }
    let submission = read(&dir.join("request.json"))?;
    same_owner(&submission["envelope"]["grant"], &original)?;
    let result = if path.join("result.json").exists() {
        read(&path.join("result.json"))?
    } else {
        json!({"status":"unknown","effect_reissued":false,"completion_confirmed":false})
    };
    let mut intent = logical(&retained["grant"]);
    intent.as_object_mut().unwrap().remove("target");
    Ok(
        json!({"operation_id":id,"target":retained["grant"]["target"],"intent":intent,"result":result}),
    )
}
fn control_using(
    dir: &Path,
    record: &Value,
    envelope: Value,
    grant: &Value,
    anchor: &Path,
    effect: impl FnOnce(&Path, &Value) -> Result<Value, String>,
) -> Result<Value, String> {
    let controls = dir.join("controls");
    directory(&controls)?;
    let id = text(grant, "operation_id")?;
    key(id)?;
    let path = controls.join(id);
    if fs::symlink_metadata(&path).is_ok() {
        existing(&path, true)?;
        let original = read(&path.join("request.json"))?;
        verify(&original, anchor)?;
        if logical(&original["grant"]) != logical(grant) {
            return Err("control operation ID already binds different signed intent/epoch; read --operation-id to reconcile; no effect reissued".into());
        }
        return control_view(dir, id, anchor);
    }
    crate::private_dir(&path)
        .map_err(|_| "control acceptance concurrent/uncertain; reconcile SAME operation ID")?;
    sync_dir(&controls)?;
    write_new(&path.join("request.json"), &envelope)?;
    write_new(&path.join("executing.json"), &json!({"started":true}))?;
    let run = record_run(record, anchor)?;
    let result = match effect(&run, &grant["target"]) {
        Ok(receipt) => {
            json!({"status":"acknowledged","completion_confirmed":receipt["status"]=="stopped","receipt":receipt})
        }
        Err(error) => {
            json!({"status":"failed_or_unknown","completion_confirmed":false,"error":error,"effect_reissued":false})
        }
    };
    write_new(&path.join("result.json"), &result)?;
    control_view(dir, id, anchor)
}
fn start_supervisor(dir: &Path) -> Result<(), String> {
    let record = read(&dir.join("request.json"))?;
    let endpoint = &record["envelope"]["grant"]["endpoint"];
    let mut paths = vec![
        Path::new(text(endpoint, "exe")?)
            .parent()
            .ok_or("missing candidate bin directory")?
            .to_owned(),
        Path::new(text(endpoint, "backend")?)
            .parent()
            .ok_or("missing backend directory")?
            .to_owned(),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    let path = std::env::join_paths(paths).map_err(|_| "invalid server launcher PATH")?;
    let mut cmd = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    cmd.arg("__remote-supervise")
        .arg(dir)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(crate::create(&dir.join("stdout.log"))?))
        .stderr(Stdio::from(crate::create(&dir.join("stderr.log"))?));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::umask(0o077);
                Ok(())
            });
        }
    }
    let _child = cmd.spawn().map_err(|e| {
        format!("supervisor startup uncertain ({e}); request retained, no automatic replay")
    })?;
    Ok(())
}
fn supervise(dir: &Path) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("remote supervisor requires Linux".into());
    }
    let (exe, anchor, user) = endpoint_identity()?;
    canonical_path(dir, true)?;
    existing(dir, true)?;
    let record = read(&dir.join("request.json"))?;
    let grant = verify(&record["envelope"], &anchor)?;
    endpoint_binding(&grant, &exe, &user)?;
    if grant["op"] != "submit" || request_dir(&grant)? != dir {
        return Err("supervisor grant is not bound to this durable submission".into());
    }
    fresh(
        &grant,
        record["accepted_at"]
            .as_u64()
            .ok_or("missing durable acceptance time")?,
    )?;
    let _owner = lock(&dir.join("supervisor.lock"))?;
    read(&dir.join("claimed.json"))?;
    write_new(&dir.join("executing.json"), &json!({"started":true}))?;
    let run = record_run(&record, &anchor)?;
    install(&grant)?;
    let invocation = invocation(&grant, Some(&run))?;
    let outcome = if invocation[0] == "run" {
        crate::parse(&invocation).and_then(|mut options| {
            if options.store != Path::new(text(&grant["endpoint"], "store")?)
                || options.cwd != Path::new(text(&grant, "cwd")?)
            {
                return Err("parsed remote paths differ from signed grant".into());
            }
            options.ctx_store = options.store.join("remote-ctx");
            options.dec_store = options.store.join("remote-dec");
            options.dec_workspace = Some(options.cwd.clone());
            lifecycle::launch_at(options, run.clone())
        })
    } else {
        lifecycle::existing(&invocation)
    };
    let revision = read(&run.join("manifest.json"))
        .ok()
        .map(|m| m["revision"].clone());
    let result = match &outcome {
        Ok(()) => json!({"ok":true,"revision":revision}),
        Err(error) => json!({"ok":false,"error":error,"revision":revision}),
    };
    write_new(&dir.join("result.json"), &result)?;
    outcome
}
fn view(dir: &Path, record: &Value, request: &Value, anchor: &Path) -> Result<Value, String> {
    let run = record_run(record, anchor)?;
    let result = if dir.join("result.json").exists() {
        read(&dir.join("result.json"))?
    } else {
        Value::Null
    };
    let owner = if dir.join("supervisor.lock").exists() {
        existing(&dir.join("supervisor.lock"), false)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join("supervisor.lock"))
            .map_err(|e| e.to_string())?;
        match file.try_lock() {
            Err(fs::TryLockError::WouldBlock) => "live",
            _ => "unknown",
        }
    } else {
        "unknown"
    };
    let mut manifest = if run.join("manifest.json").exists() {
        read(&run.join("manifest.json"))?
    } else {
        Value::Null
    };
    if manifest["allow_subteam"] == true {
        manifest["descendants"] = crate::subteam::summary(&run)?;
    }
    let accounting_gap = owner == "unknown" && result.is_null();
    let (target, target_error) = match crate::app_server::control_target(&run) {
        Ok(target) => (Some(target), None),
        Err(error) => (None, Some(error)),
    };
    let mut view = json!({"request_id":record["envelope"]["grant"]["request_id"],"run_id":run.file_name().unwrap().to_string_lossy(),
        "accepted":true,"supervisor":owner,"operation_result":result,"manifest":manifest,
        "control_target":target,"control_target_error":target_error,"accounting_may_be_incomplete":accounting_gap});
    if request["artifact"] == true || request["file"].is_string() {
        let relative = if request["artifact"] == true {
            storage::artifact_path(&run)?
        } else {
            PathBuf::from(text(request, "file")?)
        };
        let path = storage::managed_path(&run, &relative)?;
        existing(&path, false)?;
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|f| f.take(LIMIT / 8 + 1).read_to_end(&mut bytes))
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > LIMIT / 8 {
            return Err("remote file read exceeds 2 MiB UTF-8 limit".into());
        }
        view["content"] =
            json!(String::from_utf8(bytes).map_err(|_| "remote read requires UTF-8")?);
    }
    Ok(view)
}

fn validate_target(target: &Value) -> Result<(), String> {
    if target.as_object().is_none_or(|o| o.len() != 4)
        || target["revision"].as_u64().is_none_or(|r| r == 0)
        || !target["subtree_epoch"].is_u64()
        || (!target["control_epoch"].is_null() && !target["control_epoch"].is_string())
        || (!target["turn_id"].is_null() && !target["turn_id"].is_string())
    {
        return Err(
            "effectful controls require a signed revision/subtree/turn/endpoint target".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        authority: PathBuf,
        anchor: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("pira-remote-fixture-{}", random().unwrap()));
            crate::private_dir(&root).unwrap();
            Self {
                authority: root.join("synthetic-key"),
                anchor: root.join("allowed-signers"),
                root,
            }
        }
        #[cfg(unix)]
        fn signing() -> Self {
            let f = Self::new();
            let status = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&f.authority)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(
                status.success(),
                "synthetic signing test requires OpenSSH ssh-keygen"
            );
            let public = public_key(&f.authority).unwrap();
            fs::write(
                &f.anchor,
                format!("fixture-root namespaces=\"{NAMESPACE}\" {public}\n"),
            )
            .unwrap();
            f
        }
        fn grant(&self, op: &str) -> Value {
            let issued = now().unwrap();
            let prefix = if cfg!(unix) {
                self.root.to_string_lossy().into_owned()
            } else {
                "/remote-fixture".to_owned()
            };
            json!({"protocol":VERSION,"scope":SCOPE,"issuer":"fixture-root","endpoint":{
                "host":"fixture-host","user":"fixture-user","endpoint_id":"1".repeat(64),
                "exe":format!("{prefix}/candidate"),"backend":format!("{prefix}/backend"),"store":format!("{prefix}/store")},
                "op":op,"request_id":"fixture-request","run_request":null,
                "args":if op=="submit" {json!(["run","--completion-gate","fixture complete","--task","fixture only"])} else {json!([op])},
                "context":if op=="submit" {json!({"caller_thread_id":"fixture-root","sandbox_policy":{"type":"danger-full-access"},"approval_policy":"never","model":"test-model","effort":"high"})} else {Value::Null},
                "issued_at":issued,"expires_at":issued+VALIDITY,"nonce":"2".repeat(64),"file":null,"artifact":false,
                "operation_id":null,"cwd":if op=="submit" {json!(prefix)} else {Value::Null},"target":null})
        }
        #[cfg(unix)]
        fn accepted(&self) -> (Value, Value, PathBuf, Value) {
            let grant = self.grant("submit");
            let envelope = sign_with(&grant, &self.authority).unwrap();
            verify(&envelope, &self.anchor).unwrap();
            submit_using(envelope.clone(), &grant, &self.anchor, |dir| {
                assert!(dir.join("request.json").is_file());
                assert!(dir.join("claimed.json").is_file());
                Ok(())
            })
            .unwrap();
            let (dir, record) = owned(&grant, &self.anchor).unwrap();
            (grant, envelope, dir, record)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn control_target(revision: u64) -> Value {
        json!({"revision":revision,"subtree_epoch":0,"control_epoch":format!("epoch-{revision}"),"turn_id":format!("turn-{revision}")})
    }
    fn control_grant(f: &Fixture, op: &str) -> Value {
        let mut grant = f.grant(op);
        grant["operation_id"] = json!("operation-one");
        grant["target"] = control_target(1);
        if op == "steer" {
            grant["context"] = f.grant("submit")["context"].clone();
            grant["args"] = json!([
                "steer",
                "--task",
                "new fixture task",
                "--completion-gate",
                "new fixture complete"
            ]);
        }
        grant
    }
    fn published_run(run: &Path, revision: u64) {
        lifecycle::save_json(
            &run.join("manifest.json"),
            &json!({"creator_main_session_id":"fixture-root","parent_run_id":null,
            "revision":revision,"subtree_epoch":0,"status":"running"}),
        )
        .unwrap();
        lifecycle::save_json(&run.join("control.json"),&json!({"revision":revision,"remote_epoch":format!("epoch-{revision}"),
            "turn_id":format!("turn-{revision}"),"token":"fixture-native-token-never-returned","address":"127.0.0.1:1"})).unwrap();
    }
    #[test]
    fn remote_posix_paths_and_host_quoting_are_not_local_path_authority() {
        for bad in ["relative", "C:\\remote", "/x/../y", "/x/./y", "/x\nnext"] {
            assert!(absolute(bad).is_err(), "{bad}");
        }
        for good in ["/remote/work space", "/remote/quote'and;literal", "/"] {
            assert!(absolute(good).is_ok());
        }
        for bad in [
            "-oProxyCommand=evil",
            "user@host;evil",
            "host\nnext",
            "host name",
            "$(evil)",
        ] {
            assert!(host(bad).is_err());
        }
        assert_eq!(quote("/tmp/a'b;echo x"), "'/tmp/a'\\''b;echo x'");
        assert!(!has_host(
            &["run", "--task", "--host", "--completion-gate", "done"].map(str::to_owned)
        ));
        assert!(!has_host(&["run", "--", "--host"].map(str::to_owned)));
        assert!(has_host(&["run", "--host=alias"].map(str::to_owned)));
    }
    #[cfg(unix)]
    #[test]
    fn signed_run_preserves_tier_selection_subteam_and_task_boundaries() {
        let f = Fixture::new();
        for (selection, expected) in [
            (vec![], crate::service_tier::Tier::Standard),
            (
                vec!["--service-tier", "fast"],
                crate::service_tier::Tier::Fast,
            ),
            (
                vec!["--service-tier", "standard"],
                crate::service_tier::Tier::Standard,
            ),
        ] {
            let mut grant = f.grant("submit");
            let mut args = vec!["run", "--allow-subteam", "--completion-gate", "fixture"];
            args.extend(selection);
            args.extend(["--", "--service-tier fast is literal task text"]);
            grant["args"] = json!(args);
            grant["context"]["service_tier"] = json!("fast");
            validate_grant(&grant).unwrap();
            let argv = invocation(&grant, None).unwrap();
            let options = crate::parse_using(&argv, || Ok(grant["context"].clone())).unwrap();
            assert_eq!(options.service_tier, expected);
            assert!(options.allow_subteam);
            assert_eq!(options.cwd, f.root);
            assert_eq!(options.task, "--service-tier fast is literal task text");
        }
        for selection in [
            vec!["--service-tier", "invalid"],
            vec!["--service-tier", "fast", "--service-tier", "standard"],
        ] {
            let mut grant = f.grant("submit");
            let mut args = vec!["run", "--completion-gate", "fixture", "--task", "fixture"];
            args.extend(selection);
            grant["args"] = json!(args);
            let argv = invocation(&grant, None).unwrap();
            assert!(crate::parse_using(&argv, || Ok(grant["context"].clone())).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn signed_paths_precede_task_terminator_and_subteam_grant_remains_explicit() {
        let f = Fixture::new();
        let mut grant = f.grant("submit");
        grant["args"] = json!([
            "run",
            "--model",
            "test-model",
            "--effort",
            "high",
            "--completion-gate",
            "fixture",
            "--allow-subteam",
            "--",
            "--cwd is task text"
        ]);
        let args = invocation(&grant, None).unwrap();
        let opt = crate::parse_using(&args, || Ok(grant["context"].clone())).unwrap();
        assert_eq!(opt.cwd, f.root);
        assert!(opt.allow_subteam);
        assert_eq!(opt.task, "--cwd is task text");
        for (name, value) in [
            ("--store", "/elsewhere"),
            ("--cwd", "/elsewhere"),
            ("--store=/elsewhere", "ignored"),
        ] {
            grant["args"] = json!(["run", name, value]);
            assert!(invocation(&grant, None).is_err());
        }
        grant["args"] = json!(["run", "--task", "--store=/task-data"]);
        assert!(invocation(&grant, None).is_ok());
    }
    #[test]
    fn policy_tightening_preserves_observation_and_stop_not_execution_or_steer() {
        let f = Fixture::new();
        let original = f.grant("submit");
        for op in ["read", "interrupt"] {
            let grant = if op == "interrupt" {
                control_grant(&f, op)
            } else {
                f.grant(op)
            };
            same_owner(&original, &grant).unwrap();
            validate_grant(&grant).unwrap();
        }
        let mut grant = control_grant(&f, "steer");
        validate_grant(&grant).unwrap();
        grant["context"]["sandbox_policy"] = json!({"type":"read-only"});
        assert!(validate_grant(&grant).is_err());
        let mut wrong = f.grant("read");
        wrong["issuer"] = json!("different-root");
        assert!(same_owner(&original, &wrong).is_err());
        let probe = f.grant("probe");
        assert!(validate_grant(&probe).is_err());
        let mut mismatch = f.grant("submit");
        mismatch["context"]["caller_thread_id"] = json!("different-root");
        assert!(validate_grant(&mismatch).is_err());
    }
    #[test]
    fn bounded_scope_expiry_and_effectful_target_are_mandatory() {
        let f = Fixture::new();
        let mut grant = f.grant("read");
        validate_grant(&grant).unwrap();
        grant["scope"] = json!("child");
        assert!(validate_grant(&grant).is_err());
        grant = f.grant("read");
        grant["protocol"] = json!(1);
        assert!(validate_grant(&grant).is_err());
        grant = f.grant("read");
        grant["expires_at"] = json!(grant["issued_at"].as_u64().unwrap() + VALIDITY + 1);
        assert!(validate_grant(&grant).is_err());
        let grant = f.grant("read");
        assert!(fresh(&grant, grant["expires_at"].as_u64().unwrap() + 1).is_err());
        for op in ["steer", "interrupt"] {
            let mut grant = control_grant(&f, op);
            grant["target"] = Value::Null;
            assert!(validate_grant(&grant).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn independent_enrollment_rejects_forgery_tampering_and_wrong_principal() {
        let f = Fixture::signing();
        let grant = f.grant("submit");
        let envelope = sign_with(&grant, &f.authority).unwrap();
        assert_eq!(verify(&envelope, &f.anchor).unwrap(), grant);
        let mut tampered = envelope.clone();
        tampered["grant"]["context"]["sandbox_policy"] = json!({"type":"read-only"});
        assert!(verify(&tampered, &f.anchor).is_err());
        let mut unsigned = envelope.clone();
        unsigned["signature"] = json!("");
        assert!(verify(&unsigned, &f.anchor).is_err());
        let mut other = grant.clone();
        other["issuer"] = json!("unknown-root");
        other["context"]["caller_thread_id"] = json!("unknown-root");
        assert!(verify(&sign_with(&other, &f.authority).unwrap(), &f.anchor).is_err());
        let unknown = Fixture::signing();
        assert!(verify(&sign_with(&grant, &unknown.authority).unwrap(), &f.anchor).is_err());
        assert!(verify(&envelope, &f.root.join("not-enrolled")).is_err());
        let public = public_key(&f.authority).unwrap();
        fs::write(
            &f.anchor,
            format!("* namespaces=\"{NAMESPACE}\" {public}\n"),
        )
        .unwrap();
        assert!(
            verify(&envelope, &f.anchor)
                .unwrap_err()
                .contains("exact root principal")
        );
        let mut protocol = envelope.clone();
        protocol["version"] = json!(1);
        assert!(verify(&protocol, &f.anchor).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn fixed_endpoint_nonce_blocks_replay_to_another_enrollment_and_wrapper() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let grant = f.grant("submit");
        for name in ["candidate", "backend"] {
            fs::write(f.root.join(name), b"fixture only").unwrap();
            fs::set_permissions(f.root.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let exe = f.root.join("candidate");
        let descriptor = appended(&exe, ".remote-endpoint.json");
        write_new(&descriptor,&json!({"version":VERSION,"endpoint_id":"1".repeat(64),"exe":exe,"user":"fixture-user"})).unwrap();
        endpoint_binding(&grant, &exe, "fixture-user").unwrap();
        let mut other = grant.clone();
        other["endpoint"]["endpoint_id"] = json!("3".repeat(64));
        assert!(endpoint_binding(&other, &exe, "fixture-user").is_err());
        assert!(endpoint_binding(&grant, &exe, "other-user").is_err());
        other = grant;
        other["endpoint"]["exe"] = json!(f.root.join("wrapper"));
        assert!(endpoint_binding(&other, &exe, "fixture-user").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn lost_submit_ack_reconciles_once_and_resume_uses_same_signed_run() {
        let f = Fixture::signing();
        let grant = f.grant("submit");
        let envelope = sign_with(&grant, &f.authority).unwrap();
        let calls = std::cell::Cell::new(0);
        assert!(
            submit_using(envelope.clone(), &grant, &f.anchor, |dir| {
                calls.set(calls.get() + 1);
                assert!(dir.join("request.json").is_file());
                assert!(dir.join("claimed.json").is_file());
                Err("fixture lost ack after acceptance".into())
            })
            .is_err()
        );
        let view = submit_using(envelope.clone(), &grant, &f.anchor, |_| {
            panic!("must not resubmit")
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(
            view["run_id"],
            format!("remote-{}", grant["nonce"].as_str().unwrap())
        );
        let mut changed = grant.clone();
        changed["args"] = json!(["run", "different task"]);
        assert!(
            submit_using(
                sign_with(&changed, &f.authority).unwrap(),
                &changed,
                &f.anchor,
                |_| panic!("mismatch must not spawn")
            )
            .is_err()
        );
        let mut resume = grant.clone();
        resume["request_id"] = json!("resume-one");
        resume["run_request"] = grant["request_id"].clone();
        resume["args"] = json!(["resume"]);
        resume["cwd"] = Value::Null;
        resume["nonce"] = json!("4".repeat(64));
        let resumed = submit_using(
            sign_with(&resume, &f.authority).unwrap(),
            &resume,
            &f.anchor,
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(resumed["run_id"], view["run_id"]);
        let repeated = submit_using(
            sign_with(&resume, &f.authority).unwrap(),
            &resume,
            &f.anchor,
            |_| panic!("must not resume twice"),
        )
        .unwrap();
        assert_eq!(repeated["run_id"], view["run_id"]);
    }
    #[cfg(unix)]
    #[test]
    fn partial_claim_expired_unknown_and_live_owner_fail_closed() {
        let f = Fixture::signing();
        let (grant, envelope, dir, record) = f.accepted();
        let owner = lock(&dir.join("supervisor.lock")).unwrap();
        assert_eq!(
            view(&dir, &record, &json!({}), &f.anchor).unwrap()["supervisor"],
            "live"
        );
        drop(owner);
        assert_eq!(
            view(&dir, &record, &json!({}), &f.anchor).unwrap()["supervisor"],
            "unknown"
        );
        let mut partial = grant.clone();
        partial["request_id"] = json!("partial");
        directory(&dir.parent().unwrap().join("partial")).unwrap();
        assert!(
            submit_using(
                sign_with(&partial, &f.authority).unwrap(),
                &partial,
                &f.anchor,
                |_| panic!("partial cannot replay")
            )
            .is_err()
        );
        let mut expired = grant.clone();
        expired["request_id"] = json!("expired");
        expired["issued_at"] = json!(1);
        expired["expires_at"] = json!(301);
        assert!(
            submit_using(
                sign_with(&expired, &f.authority).unwrap(),
                &expired,
                &f.anchor,
                |_| panic!("expired unknown cannot launch")
            )
            .is_err()
        );
        assert!(!dir.parent().unwrap().join("expired").exists());
        assert!(
            submit_using(envelope, &grant, &f.anchor, |_| panic!(
                "accepted never replay"
            ))
            .is_ok()
        );
    }
    #[cfg(unix)]
    #[test]
    fn control_ack_loss_and_crash_reconcile_without_effect_even_after_resume() {
        let f = Fixture::signing();
        let (_, _, dir, record) = f.accepted();
        let run = record_run(&record, &f.anchor).unwrap();
        published_run(&run, 1);
        let mut grant = control_grant(&f, "interrupt");
        let envelope = sign_with(&grant, &f.authority).unwrap();
        let calls = std::cell::Cell::new(0);
        let first = control_using(&dir, &record, envelope, &grant, &f.anchor, |_, _| {
            calls.set(calls.get() + 1);
            Ok(json!({"status":"accepted"}))
        })
        .unwrap();
        assert_eq!(first["result"]["completion_confirmed"], false); // acknowledgement is not worker completion
        published_run(&run, 2);
        grant["target"] = control_target(2);
        grant["nonce"] = json!("5".repeat(64));
        assert!(
            control_using(
                &dir,
                &record,
                sign_with(&grant, &f.authority).unwrap(),
                &grant,
                &f.anchor,
                |_, _| panic!("changed target must not affect resumed run"),
            )
            .unwrap_err()
            .contains("epoch")
        );
        let retry = control_view(&dir, "operation-one", &f.anchor).unwrap();
        assert_eq!(first, retry);
        assert_eq!(calls.get(), 1);
        grant["operation_id"] = json!("crashed-control");
        let path = dir.join("controls/crashed-control");
        directory(&path).unwrap();
        write_new(
            &path.join("request.json"),
            &sign_with(&grant, &f.authority).unwrap(),
        )
        .unwrap();
        write_new(&path.join("executing.json"), &json!({"started":true})).unwrap();
        let unknown = control_using(
            &dir,
            &record,
            sign_with(&grant, &f.authority).unwrap(),
            &grant,
            &f.anchor,
            |_, _| panic!("crashed control cannot replay"),
        )
        .unwrap();
        assert_eq!(unknown["result"]["status"], "unknown");
        assert_eq!(unknown["result"]["effect_reissued"], false);
    }
    #[cfg(unix)]
    #[test]
    fn stale_signed_steer_and_interrupt_cannot_reach_new_native_revision() {
        let f = Fixture::signing();
        let (_, _, _, record) = f.accepted();
        let run = record_run(&record, &f.anchor).unwrap();
        published_run(&run, 2);
        let old = control_target(1);
        for op in ["steer", "interrupt"] {
            assert!(
                crate::app_server::control_bound(&run, op, "fixture", Some("fixture"), &old)
                    .unwrap_err()
                    .contains("stale")
            );
        }
        lifecycle::save_json(&run.join("tree.json"),&json!({"version":1,"epoch":0,"blocked":false,"members":{
            run.file_name().unwrap().to_str().unwrap():{"active":false,"revision":2,"blocked":false}}})).unwrap();
        assert!(
            crate::subteam::interrupt_bound(&run, true, &old)
                .unwrap_err()
                .contains("stale")
        );
        assert_eq!(read(&run.join("tree.json")).unwrap()["blocked"], false);
        let target = crate::app_server::control_target(&run).unwrap();
        assert!(target.get("token").is_none());
    }
    #[cfg(unix)]
    #[test]
    fn stored_unsigned_run_pointer_and_unsafe_ancestor_never_override_grant() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::signing();
        let (_, _, _, mut record) = f.accepted();
        let mut query = f.grant("read");
        query["request_id"] = json!("copied-ledger");
        let copied =
            Path::new(text(&query["endpoint"], "store").unwrap()).join(".remote-v2/copied-ledger");
        directory(&copied).unwrap();
        write_new(&copied.join("request.json"), &record).unwrap();
        assert!(
            owned(&query, &f.anchor)
                .unwrap_err()
                .contains("ledger location")
        );
        let other = f.root.join("store/other-run");
        directory(&other).unwrap();
        record["run"] = json!(other);
        assert!(record_run(&record, &f.anchor).is_err());
        let unsafe_parent = f.root.join("unsafe");
        directory(&unsafe_parent).unwrap();
        let leaf = unsafe_parent.join("private");
        directory(&leaf).unwrap();
        fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(existing(&leaf, true).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn verified_context_is_process_private_and_not_inherited_by_child_threads() {
        let f = Fixture::signing();
        let grant = f.grant("submit");
        let verified = verify(&sign_with(&grant, &f.authority).unwrap(), &f.anchor).unwrap();
        assert!(delegated_context().is_none());
        install(&verified).unwrap();
        assert_eq!(profile::caller_session().unwrap(), "fixture-root");
        assert!(
            std::thread::spawn(|| delegated_context().is_none())
                .join()
                .unwrap()
        );
        DELEGATION.with(|slot| *slot.borrow_mut() = None);
        assert!(delegated_context().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn signed_fresh_steer_policy_must_match_native_endpoint_policy() {
        let f = Fixture::signing();
        let (_, _, _, record) = f.accepted();
        let run = record_run(&record, &f.anchor).unwrap();
        published_run(&run, 1);
        let mut endpoint = read(&run.join("control.json")).unwrap();
        endpoint["sandbox_policy"] = json!({"type":"workspaceWrite"});
        lifecycle::save_json(&run.join("control.json"), &endpoint).unwrap();
        let grant = control_grant(&f, "steer");
        let verified = verify(&sign_with(&grant, &f.authority).unwrap(), &f.anchor).unwrap();
        install(&verified).unwrap();
        let error = crate::app_server::control_bound(
            &run,
            "steer",
            "fixture",
            Some("fixture"),
            &grant["target"],
        )
        .unwrap_err();
        DELEGATION.with(|slot| *slot.borrow_mut() = None);
        assert!(error.contains("permissions differ"), "{error}");
    }
    #[test]
    fn negotiation_requires_exact_protocol_scope_and_operation_capabilities() {
        let metadata = capability_metadata();
        for operation in [
            "run",
            "resume",
            "read",
            "attach",
            "steer",
            "interrupt",
            "remote-check",
        ] {
            require_capabilities(&metadata, operation, false).unwrap();
        }
        let mut bad = metadata.clone();
        bad["protocol"] = json!(1);
        assert!(
            require_capabilities(&bad, "run", false)
                .unwrap_err()
                .contains("protocol")
        );
        bad = metadata.clone();
        bad["scope"] = json!("different-root-scope");
        assert!(
            require_capabilities(&bad, "run", false)
                .unwrap_err()
                .contains("scope")
        );
        assert!(
            require_capabilities(&json!({"protocol":VERSION,"capabilities":[]}), "run", false)
                .unwrap_err()
                .contains("capabilit")
        );
        for (operation, capability) in [
            ("run", "durable-submit"),
            ("resume", "resume"),
            ("steer", "steer"),
            ("interrupt", "interrupt"),
            ("attach", "read"),
        ] {
            let mut limited = metadata.clone();
            limited["capabilities"]
                .as_array_mut()
                .unwrap()
                .retain(|v| v != capability);
            assert!(
                require_capabilities(&limited, operation, false)
                    .unwrap_err()
                    .contains(capability)
            );
        }
        let mut ordinary = metadata;
        ordinary["capabilities"]
            .as_array_mut()
            .unwrap()
            .retain(|v| v != "host-pinned-subteam");
        require_capabilities(&ordinary, "run", false).unwrap();
        assert!(
            require_capabilities(&ordinary, "run", true)
                .unwrap_err()
                .contains("host-pinned-subteam")
        );
        assert!(!requests_subteam(
            &["run", "--task", "--allow-subteam"].map(str::to_owned)
        ));
        assert!(!requests_subteam(
            &["run", "--", "--allow-subteam"].map(str::to_owned)
        ));
        assert!(requests_subteam(
            &["run", "--allow-subteam", "--task", "fixture"].map(str::to_owned)
        ));
    }
    #[test]
    fn capability_metadata_request_is_identity_only_and_cannot_select_effects() {
        let f = Fixture::new();
        let cli = Client {
            endpoint: f.grant("read")["endpoint"].clone(),
            authority: f.authority.clone(),
            state: f.root.join("missing-state"),
            id: "new-request".into(),
            run_request: Some("original".into()),
            operation: Some("stop-id".into()),
            cwd: Some("/server/work".into()),
            detach: false,
            artifact: true,
            file: Some("manifest.json".into()),
            args: vec!["interrupt".into()],
        };
        let grant = cli.capability_request("fixture-root").unwrap();
        assert_eq!(grant["args"], json!(["capabilities"]));
        assert_eq!(grant["op"], "read");
        for field in [
            "context",
            "cwd",
            "run_request",
            "operation_id",
            "file",
            "target",
        ] {
            assert!(grant[field].is_null(), "{field}");
        }
        assert_eq!(grant["artifact"], false);
        assert!(!cli.state.exists());
        let mut invalid = grant;
        invalid["operation_id"] = json!("stop-id");
        assert!(validate_grant(&invalid).is_err());
        invalid["operation_id"] = Value::Null;
        invalid["context"] = f.grant("submit")["context"].clone();
        assert!(validate_grant(&invalid).is_err());
    }
    #[test]
    fn resume_negotiates_accepted_promotions_not_only_initial_or_local_argv() {
        let f = Fixture::new();
        let original = f.grant("submit");
        let run = format!("remote-{}", original["nonce"].as_str().unwrap());
        let mut retained = json!({"accepted":true,"request_id":original["request_id"],"run_id":run,"manifest":{"run_id":run,"creator_main_session_id":original["issuer"],"parent_run_id":null,"allow_subteam":false}});
        let mut limited = capability_metadata();
        limited["capabilities"]
            .as_array_mut()
            .unwrap()
            .retain(|value| value != "host-pinned-subteam");
        // Initial ordinary run. An unaccepted local promotion intention is not
        // authoritative state; B still negotiates ordinary against this manifest.
        let resume_b = vec!["resume".into()];
        require_resume_capabilities(&limited, &retained, &original, &resume_b).unwrap();
        // Resume A deliberately promotes: it needs the capability even before
        // its manifest changes. Once A is accepted, B inherits that topology.
        let resume_a = vec!["resume".into(), "--allow-subteam".into()];
        assert!(
            require_resume_capabilities(&limited, &retained, &original, &resume_a)
                .unwrap_err()
                .contains("host-pinned-subteam")
        );
        require_resume_capabilities(&capability_metadata(), &retained, &original, &resume_a)
            .unwrap();
        retained["manifest"]["allow_subteam"] = json!(true);
        assert!(!requests_subteam(&strings(&original["args"]).unwrap()));
        assert!(!requests_subteam(&resume_b));
        assert!(
            require_resume_capabilities(&limited, &retained, &original, &resume_b)
                .unwrap_err()
                .contains("host-pinned-subteam")
        );
        require_resume_capabilities(&capability_metadata(), &retained, &original, &resume_b)
            .unwrap();
        let mut retried_original = original.clone();
        retried_original["nonce"] = json!("6".repeat(64));
        require_resume_capabilities(
            &capability_metadata(),
            &retained,
            &retried_original,
            &resume_b,
        )
        .unwrap();
        retained["manifest"]
            .as_object_mut()
            .unwrap()
            .remove("allow_subteam");
        assert!(
            require_resume_capabilities(&capability_metadata(), &retained, &original, &resume_b)
                .unwrap_err()
                .contains("unknown retained topology")
        );
        retained["manifest"]["allow_subteam"] = json!(false);
        let mut wrong_request = retained.clone();
        wrong_request["request_id"] = json!("other-request");
        assert!(
            require_resume_capabilities(
                &capability_metadata(),
                &wrong_request,
                &original,
                &resume_b
            )
            .is_err()
        );
        retained["manifest"]["creator_main_session_id"] = json!("wrong-root");
        assert!(
            require_resume_capabilities(&capability_metadata(), &retained, &original, &resume_b)
                .is_err()
        );
    }
    #[cfg(unix)]
    include!("remote_client_persistence_tests.rs");
}
