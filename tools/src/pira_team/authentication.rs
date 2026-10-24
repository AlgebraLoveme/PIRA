//! Selected backend credentials stay in launcher memory and backend environments.
//! The ordinary control capability cannot redeem a key: the OS peer must be the
//! admitted Team launcher, still beneath this coordinator's current backend.
use crate::{lifecycle::read_json, native, subteam};
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const BUDGET: Duration = Duration::from_secs(3);
const HANDOFF_LIMIT: usize = 1024 * 1024;

// No Debug/Serialize implementation: never include credential values in diagnostics.
pub struct Secret(Vec<u8>);
impl Secret {
    fn new(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > HANDOFF_LIMIT || std::str::from_utf8(&bytes).is_err() {
            return Err("invalid selected backend credential encoding/size".into());
        }
        Ok(Self(bytes))
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            unsafe {
                std::ptr::write_volatile(byte, 0);
            }
        }
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
    }
}

// Backend selectors are retained separately from ordinary task/build variables.
// These values travel only on the private peer-verified channel, never argv/logs.
fn backend_variable(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    if ["CODEX_HOME", "CODEX_THREAD_ID", "CODEX_SESSION_ID"].contains(&name.as_str()) {
        return false;
    }
    name.starts_with("CODEX_")
        || name.starts_with("OPENAI_")
        || name.starts_with("AZURE_OPENAI_")
        || [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "CURL_CA_BUNDLE",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "LD_AUDIT",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "DYLD_FRAMEWORK_PATH",
            "DYLD_FORCE_FLAT_NAMESPACE",
            "PYTHONHOME",
            "PYTHONPATH",
            "PYTHONSTARTUP",
        ]
        .contains(&name.as_str())
}
fn api_key_name(name: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case("CODEX_API_KEY")
    } else {
        name == "CODEX_API_KEY"
    }
}

fn file_at(home: Option<PathBuf>) -> Result<Option<PathBuf>, String> {
    let Some(home) = home else {
        return Ok(None);
    };
    match home.join("auth.json").canonicalize() {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("cannot resolve selected file authentication location".into()),
    }
}

pub fn caller_file() -> Result<Option<PathBuf>, String> {
    if std::env::var_os("CODEX_API_KEY").is_some_and(|key| !key.is_empty()) {
        return Ok(None);
    }
    let home = std::env::var_os("CODEX_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|s| !s.is_empty())
                .or_else(|| std::env::var_os("USERPROFILE").filter(|s| !s.is_empty()))
                .map(|path| PathBuf::from(path).join(".codex"))
        });
    file_at(home)
}

pub struct Backend {
    program: PathBuf,
    environment: Vec<(String, String)>,
    file: Option<PathBuf>,
}
impl Backend {
    fn capture(program: PathBuf) -> Result<Self, String> {
        let environment = std::env::vars_os()
            .filter_map(|(name, value)| {
                let name = name.into_string().ok()?;
                backend_variable(&name).then_some((name, value))
            })
            .map(|(name, value)| {
                value
                    .into_string()
                    .map(|value| (name, value))
                    .map_err(|_| "backend selector must be UTF-8".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Windows hard links do not canonicalize to their original source. Retain
        // the caller-selected path, never the coordinator's disposable home link.
        let file = caller_file()?;
        Ok(Self {
            program,
            environment,
            file,
        })
    }
    fn encode(&self) -> Result<Secret, String> {
        Secret::new(
            serde_json::to_vec(
                &json!({"program":self.program,"environment":self.environment,"file":self.file}),
            )
            .map_err(|_| "cannot encode selected backend")?,
        )
    }
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| "invalid selected backend handoff")?;
        let program = value["program"]
            .as_str()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("invalid selected backend program")?;
        let environment: Vec<(String, String)> =
            serde_json::from_value(value["environment"].clone())
                .map_err(|_| "invalid selected backend selectors")?;
        let mut names = std::collections::HashSet::new();
        if environment.iter().any(|(name, value)| {
            !backend_variable(name)
                || !names.insert(name.clone())
                || name.contains(['\0', '='])
                || value.contains('\0')
        }) {
            return Err("invalid selected backend selector names".into());
        }
        let file = match &value["file"] {
            Value::Null => None,
            Value::String(path) if Path::new(path).is_absolute() => Some(PathBuf::from(path)),
            _ => return Err("invalid selected file authentication location".into()),
        };
        Ok(Self {
            program,
            environment,
            file,
        })
    }
    fn uses_api_key(&self) -> bool {
        self.environment
            .iter()
            .any(|(name, value)| api_key_name(name) && !value.is_empty())
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        for (_, value) in &mut self.environment {
            for byte in unsafe { value.as_bytes_mut() } {
                unsafe {
                    std::ptr::write_volatile(byte, 0);
                }
            }
        }
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
    }
}
pub enum Authentication {
    Caller,
    SelectedCaller(PathBuf),
    Delegated(Backend),
}
impl Authentication {
    pub fn uses_api_key(&self) -> bool {
        match self {
            Self::Caller | Self::SelectedCaller(_) => {
                std::env::var_os("CODEX_API_KEY").is_some_and(|key| !key.is_empty())
            }
            Self::Delegated(backend) => backend.uses_api_key(),
        }
    }
    pub fn program(&self) -> &std::ffi::OsStr {
        match self {
            Self::Caller => std::ffi::OsStr::new("codex"),
            Self::SelectedCaller(program) => program.as_os_str(),
            Self::Delegated(backend) => backend.program.as_os_str(),
        }
    }
    pub fn file_source(&self) -> Option<&Path> {
        match self {
            Self::Delegated(backend) => backend.file.as_deref(),
            _ => None,
        }
    }
    pub fn apply(&self, command: &mut std::process::Command) {
        if let Self::Delegated(backend) = self {
            for (name, _) in std::env::vars_os() {
                if backend_variable(&name.to_string_lossy()) {
                    command.env_remove(name);
                }
            }
            for (name, value) in &backend.environment {
                command.env(name, value);
            }
        }
    }
}

#[cfg(unix)]
fn resolve_program() -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let paths = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    for directory in std::env::split_paths(&paths) {
        let program = directory.join("codex");
        if program.is_file()
            && program
                .metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
        {
            return program
                .canonicalize()
                .map_err(|_| "cannot retain selected Codex executable".into());
        }
    }
    Err("native Codex executable is unavailable on PATH".into())
}
#[cfg(windows)]
fn resolve_program() -> Result<PathBuf, String> {
    native::windows::resolve_program().map_err(|_| "cannot retain selected Codex executable".into())
}

fn read_frame(stream: &mut impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header) as usize;
    if size > limit {
        return Err(io::Error::other("local handoff frame exceeds limit"));
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn write_frame(stream: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()
}

pub fn inherit(run: &Path, manifest: &Value) -> Result<Authentication, String> {
    let Some(parent) = subteam::caller_run()? else {
        return Ok(Authentication::SelectedCaller(resolve_program()?));
    };
    let runtime = subteam::caller_runtime()?.ok_or("missing coordinator runtime")?;
    let control = read_json(&parent.join("control.json"))?;
    let selected = control.get("authentication").ok_or(
        "coordinator lacks launcher authentication handoff; interrupt/resume it with current Team",
    )?;
    if selected["revision"] != runtime["revision"] {
        return Err("stale coordinator authentication endpoint".into());
    }
    let endpoint = &selected["endpoint"];
    if endpoint.is_null() {
        return Err(
            "coordinator lacks selected-backend handoff; interrupt/resume it with current Team"
                .into(),
        );
    }
    let mut connection = local::connect(endpoint, Instant::now() + BUDGET)
        .map_err(|_| "launcher authentication connection unavailable")?;
    let peer = connection
        .peer()
        .map_err(|_| "cannot verify authentication server peer")?;
    let owner =
        native::identity(peer).map_err(|_| "cannot verify authentication server identity")?;
    if runtime["launcher_pid"] != peer || runtime["launcher_birth"] != owner.birth || !owner.alive {
        return Err("authentication endpoint is not the retained coordinator launcher".into());
    }
    let request = json!({"child":run.file_name().unwrap().to_str().unwrap(), "revision":manifest["revision"],
        "epoch":manifest["subtree_epoch"], "runtime":runtime});
    write_frame(&mut connection, request.to_string().as_bytes())
        .map_err(|_| "launcher authentication delivery failed")?;
    let mut status = [0_u8; 1];
    connection
        .read_exact(&mut status)
        .map_err(|_| "launcher authentication response unavailable")?;
    if status != [1] {
        return Err("launcher authentication request rejected".into());
    }
    let payload = read_frame(&mut connection, HANDOFF_LIMIT).map_err(|error| {
        format!(
            "launcher authentication response invalid or closed ({:?})",
            error.kind()
        )
    })?;
    let payload = Secret::new(payload)?;
    let backend = Backend::decode(&payload.0)?;
    // Keep a Windows pipe connected until its buffered response has been read.
    // Parent exit after delivery does not revoke this already admitted child.
    let _ = connection.write_all(&[1]);
    Ok(Authentication::Delegated(backend))
}

pub struct Server {
    endpoint: Value,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    pub fn start(
        run: &Path,
        revision: &Value,
        authentication: &Authentication,
    ) -> Result<Self, String> {
        let program = PathBuf::from(authentication.program());
        if !program.is_absolute() {
            return Err("coordinator backend selection is not retained".into());
        }
        let payload = Backend::capture(program)?.encode()?;
        let run = run.to_owned();
        let revision = revision.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let (ready, receiver) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let listener = match local::Listener::bind(&run) {
                Ok(listener) => listener,
                Err(_) => {
                    let _ = ready.send(Err(
                        "cannot create protected launcher authentication endpoint".to_owned(),
                    ));
                    return;
                }
            };
            if ready.send(Ok(listener.endpoint())).is_err() {
                return;
            }
            while !stop.load(Ordering::SeqCst) {
                let mut connection = match listener.accept(Instant::now() + BUDGET) {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                    Err(_) => break,
                };
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let accepted = (|| -> Result<(), String> {
                    let peer = connection.peer().map_err(|_| "unverified launcher peer")?;
                    let bytes = read_frame(&mut connection, 4096)
                        .map_err(|_| "invalid launcher request")?;
                    let request: Value =
                        serde_json::from_slice(&bytes).map_err(|_| "invalid launcher request")?;
                    subteam::claim_authentication(&run, &revision, peer, &request)
                })();
                if accepted.is_err() {
                    let _ = connection.write_all(&[0]);
                    continue;
                }
                // Never log this response or attach it to control/turn events.
                if connection.write_all(&[1]).is_ok()
                    && write_frame(&mut connection, &payload.0).is_ok()
                {
                    let mut delivered = [0_u8; 1];
                    let _ = connection.read_exact(&mut delivered);
                }
            }
        });
        let endpoint = receiver
            .recv()
            .map_err(|_| "authentication endpoint startup failed")??;
        Ok(Self {
            endpoint,
            stopped,
            thread: Some(thread),
        })
    }
    pub fn endpoint(&self) -> Value {
        self.endpoint.clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = local::connect(&self.endpoint, Instant::now() + BUDGET);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

mod local {
    use super::*;
    #[cfg(any(target_os = "linux", windows))]
    fn random_name() -> io::Result<String> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|_| io::Error::other("local endpoint entropy unavailable"))?;
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }
    #[cfg(target_os = "macos")]
    use std::fs::File;
    #[cfg(target_os = "linux")]
    use std::os::linux::net::SocketAddrExt;
    #[cfg(unix)]
    use std::os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    };

    #[cfg(target_os = "macos")]
    fn in_directory<T>(root: &Path, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        unsafe extern "C" {
            fn pthread_fchdir_np(fd: i32) -> i32;
        }
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                unsafe {
                    pthread_fchdir_np(-1);
                }
            }
        }
        let directory = File::open(root)?;
        if unsafe { pthread_fchdir_np(directory.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let _reset = Reset;
        operation()
    }
    #[cfg(unix)]
    pub struct Connection {
        stream: UnixStream,
        deadline: Instant,
    }
    #[cfg(unix)]
    impl Connection {
        fn ready(&self, events: i16) -> io::Result<()> {
            loop {
                let remaining = self.deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "local handoff deadline exceeded",
                    ));
                }
                let mut fd = libc::pollfd {
                    fd: self.stream.as_raw_fd(),
                    events,
                    revents: 0,
                };
                let milliseconds = remaining
                    .as_millis()
                    .saturating_add(1)
                    .min(i32::MAX as u128) as i32;
                let result = unsafe { libc::poll(&mut fd, 1, milliseconds) };
                if result > 0 {
                    return Ok(());
                }
                if result < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
            }
        }
        pub fn peer(&self) -> io::Result<u32> {
            let fd = self.stream.as_raw_fd();
            #[cfg(target_os = "linux")]
            {
                let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
                let mut length = std::mem::size_of_val(&peer) as libc::socklen_t;
                if unsafe {
                    libc::getsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_PEERCRED,
                        (&mut peer as *mut libc::ucred).cast(),
                        &mut length,
                    )
                } != 0
                {
                    return Err(io::Error::last_os_error());
                }
                if peer.uid != unsafe { libc::geteuid() } || peer.pid <= 0 {
                    return Err(io::Error::other("untrusted local peer"));
                }
                Ok(peer.pid as u32)
            }
            #[cfg(target_os = "macos")]
            {
                let (mut uid, mut gid) = (0, 0);
                if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0
                    || uid != unsafe { libc::geteuid() }
                {
                    return Err(io::Error::other("untrusted local peer"));
                }
                let mut pid = 0_i32;
                let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
                if unsafe { libc::getsockopt(fd, 0, 2, (&mut pid as *mut i32).cast(), &mut length) }
                    != 0
                    || pid <= 0
                {
                    return Err(io::Error::other("unverified local peer PID"));
                }
                Ok(pid as u32)
            }
        }
    }
    #[cfg(unix)]
    impl Read for Connection {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            loop {
                self.ready(libc::POLLIN)?;
                match self.stream.read(buffer) {
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        continue;
                    }
                    result => return result,
                }
            }
        }
    }
    #[cfg(unix)]
    impl Write for Connection {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            loop {
                self.ready(libc::POLLOUT)?;
                match self.stream.write(buffer) {
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        continue;
                    }
                    result => return result,
                }
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            self.stream.flush()
        }
    }
    #[cfg(unix)]
    pub struct Listener {
        listener: UnixListener,
        endpoint: Value,
        cleanup: Option<PathBuf>,
    }
    #[cfg(unix)]
    impl Listener {
        pub fn bind(_run: &Path) -> io::Result<Self> {
            #[cfg(target_os = "linux")]
            let (listener, endpoint, cleanup) = {
                let name = format!("pira-team-{}-{}", std::process::id(), random_name()?);
                let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
                (
                    UnixListener::bind_addr(&address)?,
                    json!({"kind":"abstract","name":name}),
                    None,
                )
            };
            #[cfg(target_os = "macos")]
            let (listener, endpoint, cleanup) = {
                use std::os::unix::fs::FileTypeExt;
                let run = _run;
                let name = "credential.sock";
                let path = run.join(name);
                if let Ok(metadata) = std::fs::symlink_metadata(&path) {
                    if !metadata.file_type().is_socket() {
                        return Err(io::Error::other(
                            "authentication endpoint path is not a socket",
                        ));
                    }
                    std::fs::remove_file(&path)?;
                }
                (
                    in_directory(run, || UnixListener::bind(name))?,
                    json!({"kind":"relative","root":run,"name":name}),
                    Some(path),
                )
            };
            Ok(Self {
                listener,
                endpoint,
                cleanup,
            })
        }
        pub fn endpoint(&self) -> Value {
            self.endpoint.clone()
        }
        pub fn accept(&self, _deadline: Instant) -> io::Result<Connection> {
            let (stream, _) = self.listener.accept()?;
            stream.set_nonblocking(true)?;
            Ok(Connection {
                stream,
                deadline: Instant::now() + BUDGET,
            })
        }
    }
    #[cfg(unix)]
    impl Drop for Listener {
        fn drop(&mut self) {
            if let Some(path) = &self.cleanup {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    #[cfg(unix)]
    pub fn connect(endpoint: &Value, deadline: Instant) -> io::Result<Connection> {
        #[cfg(target_os = "linux")]
        let stream = {
            if endpoint["kind"] != "abstract" {
                return Err(io::Error::other("invalid local endpoint kind"));
            }
            let name = endpoint["name"]
                .as_str()
                .ok_or_else(|| io::Error::other("invalid local endpoint"))?;
            UnixStream::connect_addr(&std::os::unix::net::SocketAddr::from_abstract_name(
                name.as_bytes(),
            )?)?
        };
        #[cfg(target_os = "macos")]
        let stream = {
            if endpoint["kind"] != "relative" || endpoint["name"] != "credential.sock" {
                return Err(io::Error::other("invalid local endpoint kind"));
            }
            let root = endpoint["root"]
                .as_str()
                .ok_or_else(|| io::Error::other("invalid local endpoint"))?;
            in_directory(Path::new(root), || UnixStream::connect("credential.sock"))?
        };
        stream.set_nonblocking(true)?;
        Ok(Connection { stream, deadline })
    }
    #[cfg(windows)]
    pub type Connection = native::windows::Pipe;
    #[cfg(windows)]
    pub struct Listener {
        listener: native::windows::Listener,
        endpoint: Value,
    }
    #[cfg(windows)]
    impl Listener {
        pub fn bind(_: &Path) -> io::Result<Self> {
            let name = format!(
                r"\\.\pipe\pira-team-auth-{}-{}",
                std::process::id(),
                random_name()?
            );
            Ok(Self {
                listener: native::windows::Listener::bind(&name)?,
                endpoint: json!({"kind":"pipe","name":name}),
            })
        }
        pub fn endpoint(&self) -> Value {
            self.endpoint.clone()
        }
        pub fn accept(&self, deadline: Instant) -> io::Result<Connection> {
            let mut connection = self.listener.accept(deadline)?;
            connection.deadline = Instant::now() + BUDGET;
            Ok(connection)
        }
    }
    #[cfg(windows)]
    pub fn connect(endpoint: &Value, deadline: Instant) -> io::Result<Connection> {
        if endpoint["kind"] != "pipe" {
            return Err(io::Error::other("invalid local endpoint kind"));
        }
        native::windows::connect(
            endpoint["name"]
                .as_str()
                .ok_or_else(|| io::Error::other("invalid local endpoint"))?,
            deadline,
        )
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn native_local_transport_verifies_peer_and_delivers_before_close() {
            let root = std::env::temp_dir().join(format!(
                "team-local-peer-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            crate::private_dir(&root).unwrap();
            let listener = Listener::bind(&root).unwrap();
            let endpoint = listener.endpoint();
            let server = std::thread::spawn(move || {
                let mut connection = listener.accept(Instant::now() + BUDGET).unwrap();
                assert_eq!(connection.peer().unwrap(), std::process::id());
                assert!(read_frame(&mut connection, 128).unwrap() == b"metadata only");
                write_frame(&mut connection, b"synthetic response").unwrap();
                let mut ack = [0];
                connection.read_exact(&mut ack).unwrap();
                assert_eq!(ack, [1]);
            });
            let mut connection = connect(&endpoint, Instant::now() + BUDGET).unwrap();
            assert_eq!(connection.peer().unwrap(), std::process::id());
            write_frame(&mut connection, b"metadata only").unwrap();
            assert!(read_frame(&mut connection, 128).unwrap() == b"synthetic response");
            connection.write_all(&[1]).unwrap();
            server.join().unwrap();
            drop(connection);
            std::fs::remove_dir_all(root).unwrap();
        }
        #[test]
        fn original_file_source_survives_coordinator_hard_link_detachment() {
            let root = std::env::temp_dir().join(format!(
                "team-auth-hardlink-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            crate::private_dir(&root).unwrap();
            let source = root.join("auth.json");
            std::fs::write(&source, b"synthetic authentication").unwrap();
            let coordinator_link = root.join("coordinator-auth.json");
            std::fs::hard_link(&source, &coordinator_link).unwrap();
            let selected = file_at(Some(root.clone())).unwrap().unwrap();
            assert_eq!(selected, source.canonicalize().unwrap());
            std::fs::remove_file(coordinator_link).unwrap();
            let child_link = root.join("child-auth.json");
            std::fs::hard_link(selected, &child_link).unwrap();
            assert_eq!(
                std::fs::read(child_link).unwrap(),
                b"synthetic authentication"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
        #[test]
        fn malformed_and_closed_frames_fail_without_values_in_errors() {
            assert!(read_frame(&mut &b"\x00\x00\x00\x04abc"[..], 128).is_err());
            assert!(read_frame(&mut &b"\x00\x01\x00\x00"[..], 128).is_err());
        }

        #[test]
        #[cfg(unix)]
        fn local_read_deadline_is_bounded_without_a_peer_response() {
            let (reader, _writer) = UnixStream::pair().unwrap();
            reader.set_nonblocking(true).unwrap();
            let mut connection = Connection {
                stream: reader,
                deadline: Instant::now() + Duration::from_millis(20),
            };
            let start = Instant::now();
            let error = connection.read(&mut [0]).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert!(start.elapsed() < Duration::from_secs(1));
        }
        #[test]
        #[cfg(unix)]
        fn buffered_reply_survives_peer_close() {
            let (reader, mut writer) = UnixStream::pair().unwrap();
            write_frame(&mut writer, b"synthetic frame").unwrap();
            drop(writer);
            reader.set_nonblocking(true).unwrap();
            let mut connection = Connection {
                stream: reader,
                deadline: Instant::now() + BUDGET,
            };
            assert!(read_frame(&mut connection, 128).unwrap() == b"synthetic frame");
        }
    }
}
