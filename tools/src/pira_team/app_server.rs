//! Minimal native Codex transport; adapted from archived Team JSON-RPC patterns.
use crate::{Options, command, create, lifecycle};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const FRAME_LIMIT: u64 = 64 * 1024;
const RPC_LIMIT: u64 = 16 * 1024 * 1024;
const PENDING_CONTROLS: usize = 64;

enum Event {
    Protocol(Value, std::ops::Range<u64>),
    Closed(String),
    TierWriteFailed(u64, String),
    Control(Value, mpsc::Sender<Value>),
}

struct WriteRequest {
    text: String,
    // Only this request's delivery failure is control-local, not a fatal fault.
    tier_id: Option<u64>,
}

fn relevant_event(value: &Value) -> bool {
    value.get("id").is_some()
        || match value["method"].as_str() {
            Some("turn/completed" | "thread/tokenUsage/updated") => true,
            Some("item/completed") => {
                let item = &value["params"]["item"];
                item["type"] == "agentMessage"
                    && (item["phase"].is_null() || item["phase"] == "final_answer")
            }
            _ => false,
        }
}

fn deliver_event(
    sender: &mpsc::SyncSender<Event>,
    mut event: Event,
    stopped: &AtomicBool,
) -> Result<(), String> {
    loop {
        if stopped.load(Ordering::SeqCst) {
            return Ok(());
        }
        match sender.try_send(event) {
            Ok(()) => return Ok(()),
            Err(mpsc::TrySendError::Full(pending)) => event = pending,
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err("app-server event receiver closed".into());
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Startup can receive notifications before an RPC reply. Replay their existing
// log range instead of retaining an unbounded queue or imposing a burst ceiling.
fn logged_event(
    log: &mut BufReader<File>,
    range: &mut std::ops::Range<u64>,
) -> Result<Option<Event>, String> {
    while range.start < range.end {
        log.seek(SeekFrom::Start(range.start))
            .map_err(|e| format!("seek app-server events: {e}"))?;
        let mut line = Vec::new();
        let size = log
            .by_ref()
            .take((range.end - range.start).min(RPC_LIMIT + 1))
            .read_until(b'\n', &mut line)
            .map_err(|e| format!("read app-server events: {e}"))?;
        if size == 0 || size as u64 > RPC_LIMIT || line.last() != Some(&b'\n') {
            return Err("invalid retained app-server frame".into());
        }
        let frame = range.start..range.start + size as u64;
        range.start = frame.end;
        let value: Value =
            serde_json::from_slice(&line).map_err(|_| "invalid retained app-server event")?;
        if relevant_event(&value) {
            return Ok(Some(Event::Protocol(value, frame)));
        }
    }
    Ok(None)
}

struct Endpoint {
    attempt_gate: crate::subteam::AttemptGate,
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    path: PathBuf,
    token: String,
    epoch: String,
    turn: Arc<std::sync::Mutex<Option<String>>>,
    sandbox: Value,
    authentication: Option<crate::authentication::Server>,
    revision: Value,
}
impl Endpoint {
    fn start(
        run: &Path,
        manifest: &Value,
        sandbox: &Value,
        sender: mpsc::SyncSender<Event>,
        mut controls: File,
        authentication: Option<crate::authentication::Server>,
    ) -> Result<Self, String> {
        let attempt_gate = crate::subteam::AttemptGate::new(run, manifest)?;
        let revision = &manifest["revision"];
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        let epoch: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let turn = Arc::new(std::sync::Mutex::new(None::<String>));
        let current_turn = turn.clone();
        let revision = revision.clone();
        let thread = std::thread::spawn({
            let token = token.clone();
            let revision = revision.clone();
            move || {
                let mut replies: Vec<std::thread::JoinHandle<()>> = Vec::new();
                for connection in listener.incoming() {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = connection else {
                        break;
                    };
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
                    replies.retain(|thread| !thread.is_finished());
                    // Bound outstanding replies without making an unacknowledged steer block
                    // acceptance of the interrupt that can stop its stalled turn.
                    let outcome = (|| -> Result<mpsc::Receiver<Value>, String> {
                        stream
                            .set_read_timeout(Some(Duration::from_millis(100)))
                            .map_err(|e| e.to_string())?;
                        let request = read_control_frame(
                            &mut stream,
                            &stopped,
                            Instant::now() + Duration::from_secs(3),
                        )?;
                        if request["token"].as_str() != Some(&token) {
                            return Err("invalid control capability".into());
                        }
                        // One extra bounded slot is reserved for authenticated interruption.
                        let capacity =
                            PENDING_CONTROLS + usize::from(request["operation"] == "interrupt");
                        if replies.len() >= capacity {
                            return Err("too many pending control acknowledgements".into());
                        }
                        let (reply, receiver) = mpsc::channel();
                        // Startup has no backend turn ID yet. Accept only authenticated
                        // interruption locally, including while native schema generation waits.
                        let turn = current_turn.lock().unwrap();
                        if turn.is_none() {
                            if !request["turn_id"].is_null()
                                || request["operation"] != "interrupt"
                                || crate::CANCELLED.load(Ordering::SeqCst)
                            {
                                return Err(
                                    "worker has no active turn, or startup is stopping".into()
                                );
                            }
                            let response = json!({"status":"accepted","operation":"interrupt","turn_id":null,"revision":revision});
                            writeln!(
                                controls,
                                "{}",
                                json!({"operation":"interrupt","turn_id":null,"response":response})
                            )
                            .map_err(|e| format!("record startup interruption: {e}"))?;
                            crate::CANCELLED.store(true, Ordering::SeqCst);
                            let _ = reply.send(response);
                        } else {
                            sender
                                .try_send(Event::Control(request, reply))
                                .map_err(|_| "run owner stopped")?;
                        }
                        Ok(receiver)
                    })();
                    match outcome {
                        Ok(receiver) => {
                            let stopped = stopped.clone();
                            replies.push(std::thread::spawn(move || {
                                let response = control_reply(receiver, &stopped);
                                let _ = writeln!(stream, "{response}");
                            }));
                        }
                        Err(error) => {
                            let response = json!({"status":"rejected","error":error});
                            let _ = writeln!(stream, "{response}");
                        }
                    }
                }
                stopped.store(true, Ordering::SeqCst);
                for reply in replies {
                    let _ = reply.join();
                }
            }
        });
        let endpoint = Self {
            attempt_gate,
            address,
            stop,
            thread: Some(thread),
            path: run.join("control.json"),
            token,
            epoch,
            turn,
            sandbox: sandbox.clone(),
            authentication,
            revision,
        };
        endpoint.publish(None)?;
        Ok(endpoint)
    }

    fn publish(&self, turn: Option<&str>) -> Result<(), String> {
        self.attempt_gate.with_open(|| self.publish_locked(turn))
    }

    fn publish_locked(&self, turn: Option<&str>) -> Result<(), String> {
        lifecycle::save_json(
            &self.path,
            &json!({"address":self.address.to_string(),"token":self.token,"turn_id":turn,
                "remote_epoch":self.epoch,"revision":self.revision,"tier_control":true,
                "sandbox_policy":self.sandbox,"authentication":{"revision":self.revision,
                    "endpoint":self.authentication.as_ref().map(crate::authentication::Server::endpoint)}}),
        )
    }

    fn activate(&self, id: &str) -> Result<(), String> {
        self.attempt_gate.with_open(|| {
            let mut turn = self.turn.lock().unwrap();
            self.publish_locked(Some(id))?;
            *turn = Some(id.to_owned());
            Ok(())
        })
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // A blocked attempt must still clean up, but never erase a newer endpoint.
        // Release the tree lock before waking/joining listener threads.
        if let Ok(_lock) = self.attempt_gate.lock() {
            if lifecycle::read_json(&self.path)
                .is_ok_and(|endpoint| endpoint["remote_epoch"] == self.epoch)
            {
                let _ = fs::remove_file(&self.path);
            }
        }
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn control_reply(receiver: mpsc::Receiver<Value>, stopped: &AtomicBool) -> Value {
    let error = loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(value) => return value,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break "run ended before acknowledgement; delivery outcome unknown";
            }
            Err(_) if stopped.load(Ordering::SeqCst) => {
                break "run ended; delivery outcome unknown";
            }
            Err(_) => {}
        }
    };
    json!({"status":"rejected","error":error})
}

// The endpoint uses short socket reads so shutdown and the total frame budget are
// checked even when a peer keeps trickling bytes. This never limits worker execution.
fn read_control_frame(
    input: &mut impl Read,
    stopped: &AtomicBool,
    deadline: Instant,
) -> Result<Value, String> {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        if stopped.load(Ordering::SeqCst) {
            return Err("run ended during control frame assembly".into());
        }
        if Instant::now() >= deadline {
            return Err("control frame assembly timed out".into());
        }
        if data.last() == Some(&b'\n') || data.len() as u64 > FRAME_LIMIT {
            return parse_frame(&data);
        }
        match input.read(&mut buffer) {
            Ok(0) => return parse_frame(&data),
            Ok(size) => {
                let end = buffer[..size]
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(size, |n| n + 1);
                data.extend_from_slice(&buffer[..end]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn read_frame(stream: &mut TcpStream) -> Result<Value, String> {
    let mut data = Vec::new();
    BufReader::new(stream)
        .take(FRAME_LIMIT + 1)
        .read_until(b'\n', &mut data)
        .map_err(|e| e.to_string())?;
    parse_frame(&data)
}

fn parse_frame(data: &[u8]) -> Result<Value, String> {
    if data.len() as u64 > FRAME_LIMIT || data.last() != Some(&b'\n') {
        return Err("invalid or oversized control frame".into());
    }
    serde_json::from_slice(data).map_err(|_| "invalid control JSON".into())
}

pub fn control(
    run: &Path,
    operation: &str,
    task: &str,
    gate: Option<&str>,
) -> Result<Value, String> {
    control_request(run, operation, task, gate).map_err(|error| {
        let Ok(manifest) = lifecycle::read_json(&run.join("manifest.json")) else {
            return format!("{error}; persisted lifecycle/revision unavailable; inspect the original invocation before explicit resume");
        };
        let status = manifest["status"].as_str().unwrap_or("unknown");
        let revision = &manifest["revision"];
        let next = match status {
            "completed" => "read the handoff; use explicit resume for further work",
            "needs_decision" | "incomplete" | "interrupted" | "timed_out" | "failed" =>
                "run stopped; inspect the handoff/diagnostics, then use explicit resume for further work",
            "running" if manifest["active_turn"].is_null() =>
                "starting or between turns (no active turn persisted); await the original invocation",
            "running" => "active turn persisted; owner may be stopping or unavailable; inspect the original invocation before explicit resume",
            _ => "inspect the original invocation before explicit resume",
        };
        format!("{error}; persisted status={status}, revision={revision}; {next}. No new turn was started by this control request")
    })
}

fn control_request(
    run: &Path,
    operation: &str,
    task: &str,
    gate: Option<&str>,
) -> Result<Value, String> {
    let endpoint = lifecycle::read_json(&run.join("control.json")).map_err(|e| {
        if operation == "tier" {
            format!("no active owner/control endpoint; run unchanged: {e}")
        } else {
            format!("run has no readable control endpoint: {e}")
        }
    })?;
    send_control(&endpoint, operation, task, gate)
}

fn send_control(
    endpoint: &Value,
    operation: &str,
    task: &str,
    gate: Option<&str>,
) -> Result<Value, String> {
    if operation == "tier" && endpoint["tier_control"] != true {
        return Err("unsupported active owner: no service-tier control; run unchanged".into());
    }
    if matches!(operation, "steer" | "tier") {
        let caller = crate::profile::Execution::from_context(&crate::profile::load()?)?;
        // Bind policy to the capability snapshot, not a manifest from another revision.
        // PIRA: require the same represented policy; policy changes need interrupt/resume.
        if endpoint["sandbox_policy"] != caller.sandbox {
            if operation == "tier" {
                return Err("current caller permissions differ from active worker policy; tier rejected, run unchanged".into());
            }
            return Err("current caller permissions differ from the active worker execution policy; interrupt then explicitly resume to inherit current permissions".into());
        }
    }
    let address: SocketAddr = endpoint["address"]
        .as_str()
        .ok_or("missing control address")?
        .parse()
        .map_err(|_| "invalid control address")?;
    if !address.ip().is_loopback() {
        return Err("control endpoint must be loopback".into());
    }
    let frame = json!({"token":endpoint["token"], "turn_id":endpoint["turn_id"], "operation":operation, "task":task,"completion_gate":gate});
    let mut frame = frame;
    if operation == "tier" {
        frame["service_tier"] = json!(task);
        frame.as_object_mut().unwrap().remove("task");
        frame.as_object_mut().unwrap().remove("completion_gate");
    }
    let encoded = frame.to_string();
    if encoded.len() as u64 >= FRAME_LIMIT {
        return Err("control input exceeds 64 KiB".into());
    }
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
        .map_err(|e| format!("control endpoint unavailable: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    writeln!(stream, "{encoded}").map_err(|e| format!("control delivery outcome unknown: {e}"))?;
    let response = read_frame(&mut stream)
        .map_err(|e| format!("control delivery outcome unknown; do not blindly retry: {e}"))?;
    if operation == "tier" {
        // Preserve structured rejected/unknown publication outcomes for the CLI.
        let mut response = response;
        if response["error"]
            .as_str()
            .is_some_and(|error| error.contains("delivery outcome unknown"))
        {
            response["status"] = json!("unknown");
        }
        return Ok(response);
    }
    if response["status"] != "accepted" {
        return Err(response["error"]
            .as_str()
            .unwrap_or("control rejected")
            .into());
    }
    Ok(response)
}

/// Public epoch only: never expose the native control capability to remote grants.
pub(super) fn control_target(run: &Path) -> Result<Value, String> {
    let manifest = lifecycle::read_json(&run.join("manifest.json"))?;
    let endpoint = match std::fs::symlink_metadata(run.join("control.json")) {
        Ok(_) => lifecycle::read_json(&run.join("control.json"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => return Err(e.to_string()),
    };
    Ok(
        json!({"revision":manifest["revision"],"subtree_epoch":manifest["subtree_epoch"],
        "control_epoch":endpoint["remote_epoch"],"turn_id":endpoint["turn_id"]}),
    )
}

pub(super) fn require_control_target(run: &Path, expected: &Value) -> Result<(), String> {
    if control_target(run)? != *expected {
        return Err(
            "stale signed control target; revision/turn/endpoint epoch changed; no effect sent"
                .into(),
        );
    }
    Ok(())
}

pub(super) fn control_bound(
    run: &Path,
    operation: &str,
    task: &str,
    gate: Option<&str>,
    expected: &Value,
) -> Result<Value, String> {
    let endpoint = lifecycle::read_json(&run.join("control.json"))?;
    let manifest = lifecycle::read_json(&run.join("manifest.json"))?;
    if expected["revision"] != manifest["revision"]
        || expected["subtree_epoch"] != manifest["subtree_epoch"]
        || expected["revision"] != endpoint["revision"]
        || expected["control_epoch"] != endpoint["remote_epoch"]
        || expected["turn_id"] != endpoint["turn_id"]
    {
        return Err("stale signed control target; no effect sent".into());
    }
    // Send using precisely this checked endpoint/token snapshot, never reread a
    // resumed endpoint after authorization. Native turn checks close send races.
    send_control(&endpoint, operation, task, gate)
}

pub struct Turn {
    pub status: String,
    pub text: Option<String>,
    pub usage: Option<Value>,
    pub error: Option<String>,
}

struct Server {
    attempt_gate: crate::subteam::AttemptGate,
    child: Child,
    scope: crate::process::Scope,
    writer: Option<mpsc::SyncSender<WriteRequest>>,
    writer_thread: Option<std::thread::JoinHandle<()>>,
    reader: Option<std::thread::JoinHandle<()>>,
    events: mpsc::Receiver<Event>,
    controls: mpsc::Receiver<Event>,
    event_log: BufReader<File>,
    pending: std::ops::Range<u64>,
    received_end: u64,
    next_id: u64,
    requests: File,
    request_schema: jsonschema::Validator,
    fault: Arc<std::sync::Mutex<Option<String>>>,
    watchdog: Option<std::thread::JoinHandle<()>>,
    finished: Option<mpsc::Sender<()>>,
    pipe_stop: Arc<AtomicBool>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    start_pending: Option<u64>,
    text: Option<String>,
    usage: Option<Value>,
}
impl Server {
    fn new(
        options: &Options,
        run: &Path,
        dir: &Path,
        handoff: &Path,
        attempt_gate: crate::subteam::AttemptGate,
        sender: mpsc::SyncSender<Event>,
        events: mpsc::Receiver<Event>,
        controls: mpsc::Receiver<Event>,
    ) -> Result<Self, String> {
        // A revision can contain multiple staged/repair attempts. Never let recovery
        // mistake a previous backend binding for a preflight or startup crash.
        match fs::remove_file(run.join("runtime.json")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("invalidate previous backend identity: {error}")),
        }
        let request_schema = crate::backend::preflight(options, run, dir)?;
        if crate::CANCELLED.load(Ordering::SeqCst) {
            return Err("interrupted before app-server launch".into());
        }
        let mut cmd = command(options, run, dir, handoff);
        crate::set_worker_launcher(&mut cmd)?;
        cmd.stdout(Stdio::piped())
            .stderr(create(&dir.join("stderr.log"))?);
        let mut log = create(&dir.join("events.jsonl"))?;
        let event_log =
            BufReader::new(File::open(dir.join("events.jsonl")).map_err(|e| e.to_string())?);
        let requests = create(&dir.join("requests.jsonl"))?;
        let (mut child, scope) =
            crate::process::spawn(&mut cmd).map_err(|e| format!("launch Codex app-server: {e}"))?;
        let pipe_stop = Arc::new(AtomicBool::new(false));
        let pipes = (|| {
            Ok::<_, String>((
                StoppablePipe::new(
                    child.stdin.take().ok_or("missing app-server stdin")?,
                    pipe_stop.clone(),
                )?,
                StoppablePipe::new(
                    child.stdout.take().ok_or("missing app-server stdout")?,
                    pipe_stop.clone(),
                )?,
            ))
        })();
        let (writer, stdout) = match pipes {
            Ok(pipes) => pipes,
            Err(error) => {
                scope.terminate();
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let fault = Arc::new(std::sync::Mutex::new(None));
        let watchdog_fault = fault.clone();
        let (finished, finish_rx) = mpsc::channel();
        let watchdog_scope = scope.clone();
        let watchdog = std::thread::spawn(move || {
            // Cover blocked stdin writes: signals and reader failures must still stop the peer.
            let grace = Duration::from_secs(5);
            let mut cutoff: Option<Instant> = None;
            loop {
                if watchdog_fault.lock().unwrap().is_some() {
                    watchdog_scope.terminate();
                    break;
                }
                if crate::CANCELLED.load(Ordering::SeqCst) {
                    cutoff.get_or_insert_with(|| Instant::now() + grace);
                }
                if cutoff.is_some_and(|deadline| Instant::now() >= deadline) {
                    watchdog_scope.terminate();
                    break;
                }
                match finish_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });
        let (writer, writer_thread) =
            request_writer(writer, fault.clone(), sender.clone(), pipe_stop.clone());
        let reader_fault = fault.clone();
        let output = sender.clone();
        let reader_stop = pipe_stop.clone();
        let reader = std::thread::spawn(move || {
            let mut input = BufReader::new(stdout);
            let mut offset = 0;
            let result = (|| -> Result<(), String> {
                loop {
                    let mut line = Vec::new();
                    let size = input
                        .by_ref()
                        .take(RPC_LIMIT + 1)
                        .read_until(b'\n', &mut line)
                        .map_err(|e| e.to_string())?;
                    if size == 0 {
                        return Err("app-server closed stdout".into());
                    }
                    if size as u64 > RPC_LIMIT {
                        return Err("app-server event exceeds 16 MiB".into());
                    }
                    log.write_all(&line).map_err(|e| e.to_string())?;
                    let frame = offset..offset + size as u64;
                    offset = frame.end;
                    let value: Value =
                        serde_json::from_slice(&line).map_err(|_| "invalid app-server event")?;
                    // Persist everything, but commentary/tool completions have no
                    // lifecycle effect. Required events retain wire order under backpressure.
                    if relevant_event(&value) {
                        deliver_event(&output, Event::Protocol(value, frame), &reader_stop)?;
                    }
                }
            })();
            if let Err(error) = result {
                *reader_fault.lock().unwrap() = Some(error.clone());
                let _ = output.try_send(Event::Closed(error));
            }
        });
        Ok(Self {
            attempt_gate,
            child,
            scope,
            writer: Some(writer),
            writer_thread: Some(writer_thread),
            reader: Some(reader),
            events,
            controls,
            event_log,
            pending: 0..0,
            received_end: 0,
            next_id: 1,
            requests,
            request_schema,
            fault,
            watchdog: Some(watchdog),
            finished: Some(finished),
            pipe_stop,
            thread_id: None,
            turn_id: None,
            start_pending: None,
            text: None,
            usage: None,
        })
    }
    fn send(&mut self, value: Value) -> Result<(), String> {
        let tier_id = if value["method"] == "turn/settings/update" {
            value["id"].as_u64()
        } else {
            None
        };
        let text = value.to_string();
        if text.len() as u64 > RPC_LIMIT {
            return Err("app-server request exceeds 16 MiB".into());
        }
        let writer = self.writer.as_ref().ok_or("app-server stdin closed")?;
        writeln!(self.requests, "{text}").map_err(|e| e.to_string())?;
        writer
            .try_send(WriteRequest { text, tier_id })
            .map_err(|_| "app-server request queue full or writer closed".into())
    }
    fn send_request(&mut self, method: &str, params: Value) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"id":id,"method":method,"params":params});
        crate::backend::validate_request(&self.request_schema, &request)?;
        self.send(request)?;
        Ok(id)
    }
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if crate::CANCELLED.load(Ordering::SeqCst) {
            return Err("interrupted during worker startup".into());
        }
        let id = if method == "turn/start" {
            // Admission is enqueueing the start request, not waiting for its ACK.
            // Never hold tree.lock across backend I/O or control-thread joins.
            self.attempt_gate
                .clone()
                .with_open(|| self.send_request(method, params))?
        } else {
            self.send_request(method, params)?
        };
        if method == "turn/start" {
            self.start_pending = Some(self.received_end);
        }
        loop {
            if crate::CANCELLED.load(Ordering::SeqCst) {
                return Err("interrupted during worker startup".into());
            }
            match self.events.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::Protocol(value, frame)) => {
                    self.received_end = frame.end;
                    if value.get("method").is_some() && value.get("id").is_some() {
                        self.send(json!({"id":value["id"],"error":{"code":-32601,"message":"unexpected request refused by coding-worker launcher"}}))?;
                        return Err("unexpected app-server request refused".into());
                    }
                    if value["id"].as_u64() == Some(id) {
                        if let Some(error) = value.get("error") {
                            return Err(format!("Codex {method}: {error}"));
                        }
                        return value
                            .get("result")
                            .cloned()
                            .ok_or("missing RPC result".into());
                    }
                    if self.pending.is_empty() {
                        self.pending.start = frame.start;
                    }
                    self.pending.end = frame.end;
                }
                Ok(Event::Closed(error) | Event::TierWriteFailed(_, error)) => return Err(error),
                Ok(Event::Control(_, reply)) => {
                    let _ = reply
                        .send(json!({"status":"rejected","error":"worker has no active turn"}));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(error) = self.fault.lock().unwrap().clone() {
                        return Err(error);
                    }
                }
                Err(_) => return Err("app-server disconnected".into()),
            }
        }
    }
    fn observe(&mut self, value: &Value) {
        let Some(thread_id) = self.thread_id.as_deref() else {
            return;
        };
        let p = &value["params"];
        if value.get("id").is_some()
            || p["threadId"].as_str() != Some(thread_id)
            || self
                .turn_id
                .as_deref()
                .is_some_and(|id| p["turnId"].as_str().is_some_and(|reported| reported != id))
        {
            return;
        }
        match value["method"].as_str() {
            Some("item/completed")
                if p["item"]["type"] == "agentMessage"
                    && (p["item"]["phase"].is_null() || p["item"]["phase"] == "final_answer") =>
            {
                self.text = p["item"]["text"].as_str().map(str::to_owned);
            }
            Some("thread/tokenUsage/updated") => {
                self.usage = Some(normalize_usage(&p["tokenUsage"]["total"]));
            }
            _ => {}
        }
    }

    fn retain_pending_evidence(&mut self) -> Result<(), String> {
        let Some(offset) = self.start_pending else {
            return Ok(());
        };
        // Before the start ACK, only evidence received after our own turn/start
        // request is attributable. After the ACK, enforce its exact turn ID too.
        // Shutdown has joined the writer of this file, including any event that
        // was logged while waiting for queue space. Never lose that failure evidence.
        let end = self
            .event_log
            .get_ref()
            .metadata()
            .map_err(|e| e.to_string())?
            .len();
        let mut range = offset..end;
        while let Some(Event::Protocol(value, _)) = logged_event(&mut self.event_log, &mut range)? {
            self.observe(&value);
        }
        Ok(())
    }
}
impl Server {
    fn shutdown(&mut self) {
        if self.finished.is_none() {
            return;
        }
        self.writer.take();
        let watchdog_scope = self.scope.clone();
        let (done, rx) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if rx.recv_timeout(Duration::from_secs(3)).is_err() {
                watchdog_scope.terminate();
            }
        });
        let _ = self.scope.wait(&mut self.child);
        let _ = done.send(());
        let _ = watchdog.join();
        // Detached descendants are not ours to kill, but must not hold our
        // reader/writer joins hostage by retaining inherited pipe descriptors.
        self.pipe_stop.store(true, Ordering::SeqCst);
        if let Some(finished) = self.finished.take() {
            let _ = finished.send(());
        }
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer_thread.take() {
            let _ = writer.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct StoppablePipe<T> {
    pipe: T,
    stop: Arc<AtomicBool>,
    drain_remaining: usize,
}
impl<T> StoppablePipe<T> {
    #[cfg(unix)]
    fn new(pipe: T, stop: Arc<AtomicBool>) -> Result<Self, String>
    where
        T: std::os::fd::AsRawFd,
    {
        let fd = pipe.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(format!(
                "configure cancellable app-server pipe: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self {
            pipe,
            stop,
            drain_remaining: RPC_LIMIT as usize + 1,
        })
    }
    #[cfg(not(unix))]
    fn new(pipe: T, stop: Arc<AtomicBool>) -> Result<Self, String> {
        // PIRA: Non-Unix retains process-tree termination for blocking pipe cleanup.
        Ok(Self {
            pipe,
            stop,
            drain_remaining: RPC_LIMIT as usize + 1,
        })
    }
}
impl<T: Read> Read for StoppablePipe<T> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let stopping = self.stop.load(Ordering::SeqCst);
            // Drain buffered output after the owned process exits, but a detached
            // writer cannot extend shutdown past the existing maximum frame size.
            let size = if stopping {
                bytes.len().min(self.drain_remaining)
            } else {
                bytes.len()
            };
            if size == 0 {
                return Ok(0);
            }
            match self.pipe.read(&mut bytes[..size]) {
                Ok(size) => {
                    if stopping {
                        self.drain_remaining -= size;
                    }
                    return Ok(size);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if stopping {
                        return Ok(0);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }
}
impl<T: Write> Write for StoppablePipe<T> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            match self.pipe.write(bytes) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.pipe.flush()
    }
}

// A stalled stdin must not block reception of authenticated interruption. Keep the
// queue bounded, including room for the endpoint's reserved interrupt slot.
fn request_writer(
    mut writer: impl Write + Send + 'static,
    fault: Arc<std::sync::Mutex<Option<String>>>,
    events: mpsc::SyncSender<Event>,
    stopped: Arc<AtomicBool>,
) -> (mpsc::SyncSender<WriteRequest>, std::thread::JoinHandle<()>) {
    let (sender, requests) = mpsc::sync_channel::<WriteRequest>(PENDING_CONTROLS + 1);
    let thread = std::thread::spawn(move || {
        while let Ok(request) = requests.recv() {
            if let Err(error) = writeln!(writer, "{}", request.text).and_then(|_| writer.flush()) {
                let error = format!("write app-server request: {error}");
                // A failed/partial frame makes all further writes unsafe. Close the
                // input queue before notifying the owner; never retry the frame.
                drop(requests);
                if let Some(id) = request.tier_id {
                    // Keep stdout and its workload alive. Delivery is unknown,
                    // including flush failures after bytes may have been accepted.
                    let _ = deliver_event(&events, Event::TierWriteFailed(id, error), &stopped);
                } else {
                    *fault.lock().unwrap() = Some(error.clone());
                    let _ = events.try_send(Event::Closed(error));
                }
                return;
            }
        }
    });
    (sender, thread)
}

pub fn turn(
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
    let (sender, events) = mpsc::sync_channel(64);
    // Protocol backpressure must not prevent admission of authenticated controls.
    let (control_sender, control_events) = mpsc::sync_channel(PENDING_CONTROLS + 1);
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
        control_events,
    )?;
    // Server owns cleanup before the fallible binding write; no separate spawn rollback.
    crate::subteam::bind(run, &manifest["revision"], &server.scope, dir)?;
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
        manifest["worker_policy_version"] = json!(crate::WORKER_POLICY_VERSION);
        if options.allow_subteam {
            manifest["coordinator_policy_version"] = json!(crate::COORDINATOR_POLICY_VERSION);
        }
        manifest["inject_review"] = json!(guidance.0);
        manifest["inject_implement"] = json!(guidance.1);
        manifest["inject_fig"] = json!(guidance.2);
        manifest["inject_write"] = json!(guidance.3);
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

fn normalize_usage(value: &Value) -> Value {
    let mut out = json!({});
    if let Some(fields) = value.as_object() {
        for (key, value) in fields {
            let mut snake = String::new();
            for c in key.chars() {
                if c.is_ascii_uppercase() {
                    snake.push('_');
                }
                snake.push(c.to_ascii_lowercase());
            }
            out[snake] = value.clone();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_events_wait_for_capacity_and_preserve_order() {
        let (sender, receiver) = mpsc::sync_channel(1);
        assert!(sender.try_send(Event::Protocol(json!(0), 0..1)).is_ok());
        let (done, finished) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            let stop = AtomicBool::new(false);
            for number in 1..=4 {
                deliver_event(&sender, Event::Protocol(json!(number), 0..1), &stop).unwrap();
            }
            done.send(()).unwrap();
        });
        assert!(matches!(
            finished.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        for number in 0..=4 {
            let Event::Protocol(value, _) = receiver.recv_timeout(Duration::from_secs(3)).unwrap()
            else {
                panic!("required event lost");
            };
            assert_eq!(value, number);
        }
        finished.recv_timeout(Duration::from_secs(3)).unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn shutdown_releases_full_protocol_queue_without_blocking_control_admission() {
        let (sender, receiver) = mpsc::sync_channel(1);
        assert!(sender.try_send(Event::Protocol(json!(0), 0..1)).is_ok());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (done, finished) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            done.send(deliver_event(
                &sender,
                Event::Protocol(json!(1), 1..2),
                &stopped,
            ))
            .unwrap();
        });
        assert!(matches!(
            finished.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        let (controls, requests) = mpsc::sync_channel(PENDING_CONTROLS + 1);
        let (reply, _) = mpsc::channel();
        assert!(
            controls
                .try_send(Event::Control(json!({"operation":"interrupt"}), reply))
                .is_ok()
        );
        assert!(matches!(requests.try_recv(), Ok(Event::Control(_, _))));
        stop.store(true, Ordering::SeqCst);
        finished
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        writer.join().unwrap();
        assert!(matches!(receiver.try_recv(), Ok(Event::Protocol(_, _))));
        assert!(receiver.try_recv().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn stopped_reader_drains_existing_bytes_without_waiting_for_peer_eof() {
        let (input, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let mut input = StoppablePipe::new(input, stop.clone()).unwrap();
        peer.write_all(b"observed\n").unwrap();
        stop.store(true, Ordering::SeqCst);
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"observed\n");
        // The other descriptor is still open: EOF came from owned shutdown.
        peer.write_all(b"late").unwrap();
        input.drain_remaining = 0;
        assert_eq!(input.read(&mut [0; 4]).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn stopped_writer_does_not_wait_for_peer_to_drain_a_full_pipe() {
        let (mut output, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        output.set_nonblocking(true).unwrap();
        loop {
            match output.write(&[0; 8192]) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                result => panic!("fill pipe: {result:?}"),
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let mut output = StoppablePipe::new(output, stop.clone()).unwrap();
        let (entered, ready) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            entered.send(()).unwrap();
            output.write_all(b"blocked")
        });
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        stop.store(true, Ordering::SeqCst);
        assert_eq!(
            thread.join().unwrap().unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn writer_preserves_wire_order_and_drains_on_shutdown() {
        struct Recorded(Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for Recorded {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let fault = Arc::new(std::sync::Mutex::new(None));
        let (events, _) = mpsc::sync_channel(1);
        let (writer, thread) = request_writer(
            Recorded(bytes.clone()),
            fault.clone(),
            events,
            Arc::new(AtomicBool::new(false)),
        );
        for text in ["initialize", "initialized", "turn/start"] {
            writer
                .try_send(WriteRequest {
                    text: text.into(),
                    tier_id: None,
                })
                .unwrap();
        }
        drop(writer);
        thread.join().unwrap();
        assert_eq!(
            *bytes.lock().unwrap(),
            b"initialize\ninitialized\nturn/start\n"
        );
        assert!(fault.lock().unwrap().is_none());
    }

    #[test]
    fn tier_flush_failure_survives_full_event_queue_and_disables_writes() {
        struct FlushFailure(Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for FlushFailure {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let fault = Arc::new(std::sync::Mutex::new(None));
        let stopped = Arc::new(AtomicBool::new(false));
        let (events, receiver) = mpsc::sync_channel(1);
        events
            .try_send(Event::Protocol(json!("already queued"), 0..1))
            .unwrap();
        let (writer, thread) =
            request_writer(FlushFailure(bytes.clone()), fault.clone(), events, stopped);
        writer
            .try_send(WriteRequest {
                text: "tier-frame".into(),
                tier_id: Some(7),
            })
            .unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
            Event::Protocol(_, _)
        ));
        let Event::TierWriteFailed(id, error) =
            receiver.recv_timeout(Duration::from_secs(3)).unwrap()
        else {
            panic!("tier failure became a fatal transport event")
        };
        assert_eq!(id, 7);
        assert!(error.contains("write app-server request"));
        thread.join().unwrap();
        assert!(fault.lock().unwrap().is_none());
        assert_eq!(*bytes.lock().unwrap(), b"tier-frame\n");
        assert!(matches!(
            writer.try_send(WriteRequest {
                text: "never retry".into(),
                tier_id: Some(8)
            }),
            Err(mpsc::TrySendError::Disconnected(_))
        ));
    }

    #[test]
    fn shutdown_releases_pending_tier_write_failure_without_fatal_fault() {
        struct Failing(mpsc::Sender<()>);
        impl Write for Failing {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.0.send(()).unwrap();
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let fault = Arc::new(std::sync::Mutex::new(None));
        let stopped = Arc::new(AtomicBool::new(false));
        let (entered, failed) = mpsc::channel();
        let (events, receiver) = mpsc::sync_channel(1);
        events
            .try_send(Event::Protocol(json!("already queued"), 0..1))
            .unwrap();
        let (writer, thread) =
            request_writer(Failing(entered), fault.clone(), events, stopped.clone());
        writer
            .try_send(WriteRequest {
                text: "tier-frame".into(),
                tier_id: Some(7),
            })
            .unwrap();
        failed.recv_timeout(Duration::from_secs(3)).unwrap();
        stopped.store(true, Ordering::SeqCst);
        thread.join().unwrap();
        assert!(fault.lock().unwrap().is_none());
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
            Event::Protocol(_, _)
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn blocked_writer_keeps_control_thread_responsive_and_reports_failure() {
        struct Blocked {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Write for Blocked {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.entered.send(()).unwrap();
                let _ = self.release.recv();
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (entered, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (events, receiver) = mpsc::sync_channel(1);
        let fault = Arc::new(std::sync::Mutex::new(None));
        let (writer, thread) = request_writer(
            Blocked {
                entered,
                release: blocked,
            },
            fault.clone(),
            events,
            Arc::new(AtomicBool::new(false)),
        );
        writer
            .try_send(WriteRequest {
                text: "steer".into(),
                tier_id: None,
            })
            .unwrap();
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        // The caller can still admit interruption while the transport is blocked.
        for _ in 0..PENDING_CONTROLS {
            writer
                .try_send(WriteRequest {
                    text: "steer".into(),
                    tier_id: None,
                })
                .unwrap();
        }
        writer
            .try_send(WriteRequest {
                text: "interrupt".into(),
                tier_id: None,
            })
            .unwrap();
        assert!(matches!(
            writer.try_send(WriteRequest {
                text: "overflow".into(),
                tier_id: None
            }),
            Err(mpsc::TrySendError::Full(_))
        ));
        release.send(()).unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
            Event::Closed(_)
        ));
        thread.join().unwrap();
        assert!(
            fault
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains("write app-server request")
        );
        assert!(
            writer
                .try_send(WriteRequest {
                    text: "closed".into(),
                    tier_id: None
                })
                .is_err()
        );
    }

    #[test]
    fn frame_assembly_checks_deadline_and_shutdown_between_reads() {
        struct StopAfterByte<'a>(&'a AtomicBool);
        impl Read for StopAfterByte<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                assert!(!self.0.swap(true, Ordering::SeqCst), "read after shutdown");
                buf[0] = b'{';
                Ok(1)
            }
        }
        let stopped = AtomicBool::new(false);
        let mut input = &b"{}\n"[..];
        assert!(
            read_control_frame(&mut input, &stopped, Instant::now())
                .unwrap_err()
                .contains("timed out")
        );
        assert_eq!(input, b"{}\n"); // No read is allowed after the total deadline.
        let deadline = Instant::now() + Duration::from_secs(3);
        assert!(
            read_control_frame(&mut StopAfterByte(&stopped), &stopped, deadline)
                .unwrap_err()
                .contains("run ended")
        );
    }

    #[test]
    fn successful_partial_read_cannot_extend_the_frame_deadline() {
        struct Trickle(Option<Instant>);
        impl Read for Trickle {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let deadline = self.0.take().expect("read again after frame deadline");
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                buf[0] = b' ';
                Ok(1)
            }
        }
        let deadline = Instant::now() + Duration::from_millis(10);
        let stopped = AtomicBool::new(false);
        assert!(
            read_control_frame(&mut Trickle(Some(deadline)), &stopped, deadline)
                .unwrap_err()
                .contains("timed out")
        );
    }

    #[test]
    fn bounded_frame_reader_preserves_wire_validation() {
        let stopped = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(3);
        assert_eq!(
            read_control_frame(&mut &b"{}\nignored"[..], &stopped, deadline).unwrap(),
            json!({})
        );
        for bytes in [&b"{}"[..], &b"invalid\n"[..], &b""[..]] {
            assert!(read_control_frame(&mut &*bytes, &stopped, deadline).is_err());
        }
        let mut limit = vec![b' '; FRAME_LIMIT as usize];
        limit[..2].copy_from_slice(b"{}");
        *limit.last_mut().unwrap() = b'\n';
        assert!(read_control_frame(&mut limit.as_slice(), &stopped, deadline).is_ok());
        limit.insert(2, b' ');
        assert!(
            read_control_frame(&mut limit.as_slice(), &stopped, deadline)
                .unwrap_err()
                .contains("oversized")
        );
    }

    #[test]
    fn control_replies_preserve_acknowledgements_and_unknown_delivery() {
        let stopped = AtomicBool::new(false);
        let (sender, receiver) = mpsc::channel();
        let accepted = json!({"status":"accepted"});
        sender.send(accepted.clone()).unwrap();
        assert_eq!(control_reply(receiver, &stopped), accepted);
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let reply = control_reply(receiver, &stopped);
        assert_eq!(reply["status"], "rejected");
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("delivery outcome unknown")
        );
        let (_sender, receiver) = mpsc::channel();
        stopped.store(true, Ordering::SeqCst);
        let reply = control_reply(receiver, &stopped);
        assert_eq!(reply["status"], "rejected");
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("delivery outcome unknown")
        );
    }
    struct AttemptFixture {
        root: PathBuf,
        run: PathBuf,
        manifest: Value,
    }
    impl AttemptFixture {
        fn new() -> Self {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).unwrap();
            let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("pira-attempt-race-{name}"));
            crate::private_dir(&root).unwrap();
            let run = root.join("fixture-run");
            crate::private_dir(&run).unwrap();
            let manifest = json!({"run_id":"fixture-run","creator_main_session_id":"fixture-root","parent_run_id":null,"revision":1,"subtree_epoch":0,"status":"running"});
            lifecycle::save_json(&run.join("manifest.json"), &manifest).unwrap();
            lifecycle::save_json(&run.join("tree.json"),&json!({"version":1,"epoch":0,"blocked":false,"members":{"fixture-run":{"active":true,"revision":1,"blocked":false}}})).unwrap();
            Self {
                root,
                run,
                manifest,
            }
        }
        fn endpoint(&self, label: &str) -> Endpoint {
            attempt_endpoint(&self.run, &self.manifest, &self.root.join(label)).unwrap()
        }
    }
    impl Drop for AttemptFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn attempt_endpoint(run: &Path, manifest: &Value, log: &Path) -> Result<Endpoint, String> {
        let (sender, _receiver) = mpsc::sync_channel(1);
        Endpoint::start(
            run,
            manifest,
            &json!({"type":"dangerFullAccess"}),
            sender,
            crate::create(log)?,
            None,
        )
    }

    #[test]
    fn recursive_gate_serializes_forced_same_revision_rollover_and_next_turn_admission() {
        let f = AttemptFixture::new();
        let a = f.endpoint("controls-A");
        a.activate("turn-A").unwrap();
        let target = control_target(&f.run).unwrap();
        let (checked, check_seen) = mpsc::channel();
        let (release, hold_check) = mpsc::channel();
        let run = f.run.clone();
        let signed = target.clone();
        let interrupt = std::thread::spawn(move || {
            crate::subteam::after_bound_check(move || {
                checked.send(()).unwrap();
                hold_check.recv_timeout(Duration::from_secs(8)).unwrap();
            });
            crate::subteam::close_bound_gate_for_test(&run, &signed)
        });
        check_seen.recv_timeout(Duration::from_secs(3)).unwrap();
        let (contended, contention_seen) = mpsc::channel();
        let run = f.run.clone();
        let manifest = f.manifest.clone();
        let log = f.root.join("controls-B");
        let started = Arc::new(AtomicBool::new(false));
        let model_started = started.clone();
        let rollover = std::thread::spawn(move || {
            crate::subteam::on_lock_contention(move || {
                contended.send(()).unwrap();
            });
            let publication = attempt_endpoint(&run, &manifest, &log);
            let published = publication.is_ok();
            drop(publication);
            let admission = crate::subteam::AttemptGate::new(&run, &manifest)
                .unwrap()
                .with_open(|| {
                    model_started.store(true, Ordering::SeqCst);
                    Ok(())
                });
            (published, admission)
        });
        // The contender has reached the real native tree lock while A's checked
        // critical section is paused. No sleeps or scheduling guess establish it.
        let contention = contention_seen.recv_timeout(Duration::from_secs(3));
        let during = control_target(&f.run).unwrap();
        release.send(()).unwrap();
        interrupt.join().unwrap().unwrap();
        let (published, admission) = rollover.join().unwrap();
        assert!(
            contention.is_ok(),
            "B did not contend on the checked tree critical section"
        );
        assert_eq!(during, target);
        assert!(!published);
        assert!(admission.is_err());
        assert!(!started.load(Ordering::SeqCst));
        assert!(a.activate("late-turn").is_err());
        assert_eq!(control_target(&f.run).unwrap(), target);
        drop(a);
        assert!(!f.run.join("control.json").exists()); // blocked owners still clear their endpoint
    }

    #[test]
    fn stale_recursive_target_cannot_gate_published_next_attempt_or_clear_its_endpoint() {
        let f = AttemptFixture::new();
        let a = f.endpoint("controls-A");
        a.activate("turn-A").unwrap();
        let target_a = control_target(&f.run).unwrap();
        let b = f.endpoint("controls-B");
        b.activate("turn-B").unwrap();
        let target_b = control_target(&f.run).unwrap();
        assert_ne!(target_a, target_b);
        drop(a);
        assert_eq!(control_target(&f.run).unwrap(), target_b);
        let error = crate::subteam::close_bound_gate_for_test(&f.run, &target_a).unwrap_err();
        assert!(error.contains("stale"), "{error}");
        assert_eq!(
            lifecycle::read_json(&f.run.join("tree.json")).unwrap()["blocked"],
            false
        );
        assert_eq!(control_target(&f.run).unwrap(), target_b);
        let gate = crate::subteam::AttemptGate::new(&f.run, &f.manifest).unwrap();
        let admitted = gate.with_open(|| Ok("B turn admitted")).unwrap();
        assert_eq!(admitted, "B turn admitted");
        crate::subteam::close_bound_gate_for_test(&f.run, &target_b).unwrap();
        assert!(
            gate.with_open(|| Err::<(), _>("must not reach a new model turn".into()))
                .unwrap_err()
                .contains("gate is closed")
        );
        assert!(b.activate("late-B-turn").is_err());
    }

    #[test]
    fn old_or_absent_owner_is_unsupported_without_touching_it() {
        let root = std::env::temp_dir().join(format!(
            "tier-old-owner-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        assert!(
            control_request(&root, "tier", "fast", None)
                .unwrap_err()
                .contains("no active owner")
        );
        let old = "{\"address\":\"127.0.0.1:1\",\"turn_id\":\"old\"}";
        fs::write(root.join("control.json"), old).unwrap();
        assert!(
            control_request(&root, "tier", "fast", None)
                .unwrap_err()
                .contains("unsupported active owner")
        );
        assert_eq!(fs::read_to_string(root.join("control.json")).unwrap(), old);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tier_endpoint_requires_capability_and_preserves_publication_outcomes() {
        let root = std::env::temp_dir().join(format!(
            "tier-endpoint-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        lifecycle::save_json(&root.join("tree.json"), &json!({"version":1,"epoch":0,"blocked":false,
            "members":{root.file_name().unwrap().to_str().unwrap():{"active":true,"revision":1,"blocked":false}}})).unwrap();
        let (sender, receiver) = mpsc::sync_channel(4);
        let endpoint = Endpoint::start(
            &root,
            &json!({"revision":1,"subtree_epoch":0,"parent_run_id":null}),
            &json!({"type":"dangerFullAccess"}),
            sender,
            create(&root.join("controls.jsonl")).unwrap(),
            None,
        )
        .unwrap();
        let exchange = |token: String, turn: &str| {
            let address = endpoint.address;
            let turn = turn.to_owned();
            std::thread::spawn(move || {
                let mut stream = TcpStream::connect(address).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                writeln!(
                    stream,
                    "{}",
                    json!({"token":token,"operation":"tier","turn_id":turn,"service_tier":"fast"})
                )
                .unwrap();
                read_frame(&mut stream).unwrap()
            })
        };
        let invalid = exchange("wrong synthetic capability".into(), "active")
            .join()
            .unwrap();
        assert_eq!(invalid["status"], "rejected");
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let startup = exchange(endpoint.token.clone(), "active").join().unwrap();
        assert_eq!(startup["status"], "rejected");
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        endpoint.activate("active").unwrap();
        for response in [
            json!({"status":"accepted","publication":"applied","actual_service_tier":null}),
            json!({"status":"rejected","publication":"targetUnavailable"}),
            json!({"status":"unknown","publication":"unknown"}),
        ] {
            let client = exchange(endpoint.token.clone(), "active");
            let Event::Control(request, reply) =
                receiver.recv_timeout(Duration::from_secs(3)).unwrap()
            else {
                panic!("not a control")
            };
            let (_, params) =
                crate::service_tier::active_update(&request, "thread", "active", false, false)
                    .unwrap();
            assert_eq!(
                params,
                json!({"threadId":"thread","turnId":"active","serviceTier":"fast"})
            );
            reply.send(response.clone()).unwrap();
            assert_eq!(client.join().unwrap(), response);
        }
        let client = exchange(endpoint.token.clone(), "previous");
        let Event::Control(request, reply) = receiver.recv_timeout(Duration::from_secs(3)).unwrap()
        else {
            panic!("not a control")
        };
        assert!(
            crate::service_tier::active_update(&request, "thread", "active", false, false).is_err()
        );
        reply
            .send(json!({"status":"rejected","error":"stale turn"}))
            .unwrap();
        assert_eq!(client.join().unwrap()["status"], "rejected");
        // The owner and endpoint remain usable after all rejected/unknown controls.
        assert!(!endpoint.stop.load(Ordering::SeqCst));
        drop(endpoint);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, unix))]
#[path = "tier_control_tests.rs"]
mod tier_control_tests;
