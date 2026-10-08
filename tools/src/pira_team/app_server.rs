//! Minimal native Codex transport; adapted from archived Team JSON-RPC patterns.
use crate::{Options, command, create, isolate, lifecycle, terminate};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
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
    Protocol(Value),
    Closed(String),
    Control(Value, mpsc::Sender<Value>),
}

struct Endpoint {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    path: PathBuf,
}
impl Endpoint {
    fn start(run: &Path, turn: &str, sender: mpsc::SyncSender<Event>) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn({
            let token = token.clone();
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
                        sender
                            .try_send(Event::Control(request, reply))
                            .map_err(|_| "run owner stopped")?;
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
            address,
            stop,
            thread: Some(thread),
            path: run.join("control.json"),
        };
        lifecycle::save_json(
            &endpoint.path,
            &json!({"address":address.to_string(),"token":token,"turn_id":turn}),
        )?;
        Ok(endpoint)
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = fs::remove_file(&self.path);
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
    let endpoint = lifecycle::read_json(&run.join("control.json"))
        .map_err(|e| format!("run has no readable control endpoint: {e}"))?;
    let address: SocketAddr = endpoint["address"]
        .as_str()
        .ok_or("missing control address")?
        .parse()
        .map_err(|_| "invalid control address")?;
    if !address.ip().is_loopback() {
        return Err("control endpoint must be loopback".into());
    }
    let frame = json!({"token":endpoint["token"], "turn_id":endpoint["turn_id"], "operation":operation, "task":task,"completion_gate":gate});
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
    if response["status"] != "accepted" {
        return Err(response["error"]
            .as_str()
            .unwrap_or("control rejected")
            .into());
    }
    Ok(response)
}

pub struct Turn {
    pub status: String,
    pub text: Option<String>,
    pub usage: Option<Value>,
    pub error: Option<String>,
}

struct Server {
    child: Child,
    writer: Option<mpsc::SyncSender<String>>,
    writer_thread: Option<std::thread::JoinHandle<()>>,
    reader: Option<std::thread::JoinHandle<()>>,
    events: mpsc::Receiver<Event>,
    sender: mpsc::SyncSender<Event>,
    pending: VecDeque<Value>,
    next_id: u64,
    requests: File,
    request_schema: jsonschema::Validator,
    fault: Arc<std::sync::Mutex<Option<String>>>,
    watchdog: Option<std::thread::JoinHandle<()>>,
    finished: Option<mpsc::Sender<()>>,
}
impl Server {
    fn new(options: &Options, run: &Path, dir: &Path, handoff: &Path) -> Result<Self, String> {
        let request_schema = crate::backend::preflight(options, run, dir)?;
        let mut cmd = command(options, run, dir, handoff);
        cmd.stdout(Stdio::piped())
            .stderr(create(&dir.join("stderr.log"))?);
        let mut log = create(&dir.join("events.jsonl"))?;
        let requests = create(&dir.join("requests.jsonl"))?;
        isolate(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("launch Codex app-server: {e}"))?;
        let writer = child.stdin.take().ok_or("missing app-server stdin")?;
        let stdout = child.stdout.take().ok_or("missing app-server stdout")?;
        let fault = Arc::new(std::sync::Mutex::new(None));
        let watchdog_fault = fault.clone();
        let (finished, finish_rx) = mpsc::channel();
        let pid = child.id();
        let watchdog = std::thread::spawn(move || {
            // Cover blocked stdin writes: signals and reader failures must still stop the peer.
            let grace = Duration::from_secs(5);
            let mut cutoff: Option<Instant> = None;
            loop {
                if watchdog_fault.lock().unwrap().is_some() {
                    terminate(pid);
                    break;
                }
                if crate::CANCELLED.load(Ordering::SeqCst) {
                    cutoff.get_or_insert_with(|| Instant::now() + grace);
                }
                if cutoff.is_some_and(|deadline| Instant::now() >= deadline) {
                    terminate(pid);
                    break;
                }
                match finish_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });
        let (sender, events) = mpsc::sync_channel(64);
        let (writer, writer_thread) = request_writer(writer, fault.clone(), sender.clone());
        let reader_fault = fault.clone();
        let output = sender.clone();
        let reader = std::thread::spawn(move || {
            let mut input = BufReader::new(stdout);
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
                    let value: Value =
                        serde_json::from_slice(&line).map_err(|_| "invalid app-server event")?;
                    // Persist all events, but only retain lifecycle messages in the control queue.
                    if value.get("id").is_some()
                        || matches!(
                            value["method"].as_str(),
                            Some("item/completed" | "turn/completed" | "thread/tokenUsage/updated")
                        )
                    {
                        output
                            .try_send(Event::Protocol(value))
                            .map_err(|_| "app-server event queue overflow or receiver closed")?;
                    }
                }
            })();
            let error = result.unwrap_err();
            *reader_fault.lock().unwrap() = Some(error.clone());
            let _ = output.try_send(Event::Closed(error));
        });
        Ok(Self {
            child,
            writer: Some(writer),
            writer_thread: Some(writer_thread),
            reader: Some(reader),
            events,
            sender,
            pending: VecDeque::new(),
            next_id: 1,
            requests,
            request_schema,
            fault,
            watchdog: Some(watchdog),
            finished: Some(finished),
        })
    }
    fn send(&mut self, value: Value) -> Result<(), String> {
        let text = value.to_string();
        if text.len() as u64 > RPC_LIMIT {
            return Err("app-server request exceeds 16 MiB".into());
        }
        writeln!(self.requests, "{text}").map_err(|e| e.to_string())?;
        self.writer
            .as_ref()
            .ok_or("app-server stdin closed")?
            .try_send(text)
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
        let id = self.send_request(method, params)?;
        loop {
            if crate::CANCELLED.load(Ordering::SeqCst) {
                return Err("interrupted during worker startup".into());
            }
            match self.events.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::Protocol(value)) => {
                    if value.get("method").is_some() && value.get("id").is_some() {
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
                    if self.pending.len() >= 64 {
                        return Err("too many pending app-server events".into());
                    }
                    self.pending.push_back(value);
                }
                Ok(Event::Closed(error)) => return Err(error),
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
}
impl Drop for Server {
    fn drop(&mut self) {
        self.writer.take();
        let pid = self.child.id();
        let (done, rx) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if rx.recv_timeout(Duration::from_secs(3)).is_err() {
                terminate(pid);
            }
        });
        let _ = self.child.wait();
        let _ = done.send(());
        let _ = watchdog.join();
        terminate(pid);
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

// A stalled stdin must not block reception of authenticated interruption. Keep the
// queue bounded, including room for the endpoint's reserved interrupt slot.
fn request_writer(
    mut writer: impl Write + Send + 'static,
    fault: Arc<std::sync::Mutex<Option<String>>>,
    events: mpsc::SyncSender<Event>,
) -> (mpsc::SyncSender<String>, std::thread::JoinHandle<()>) {
    let (sender, requests) = mpsc::sync_channel::<String>(PENDING_CONTROLS + 1);
    let thread = std::thread::spawn(move || {
        for text in requests {
            if let Err(error) = writeln!(writer, "{text}").and_then(|_| writer.flush()) {
                let error = format!("write app-server request: {error}");
                *fault.lock().unwrap() = Some(error.clone());
                let _ = events.try_send(Event::Closed(error));
                break;
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
    guidance: (bool, bool),
) -> Result<Turn, String> {
    let build_roots = crate::build_roots::configured()?;
    for root in &build_roots {
        options.execution.require_writable(root)?;
    }
    options.execution.require_writable(handoff)?;
    manifest["build_roots"] = json!(build_roots);
    let repair = manifest["repairs"].as_u64().ok_or("missing repair state")? > 0;
    let mut server = Server::new(options, run, dir, handoff)?;
    server.request("initialize", json!({"clientInfo":{"name":"pira_team","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":false}}))
        .map_err(|e| format!("native Codex strict-config stdio initialization failed: {e}; inspect stderr.log and update Codex/Team to compatible builds. This is separate from the interactive shared daemon"))?;
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
    manifest["inject_review"] = json!(guidance.0);
    manifest["inject_implement"] = json!(guidance.1);
    lifecycle::save_json(&run.join("manifest.json"), manifest)?;
    let started = server.request(
        "turn/start",
        json!({"threadId":thread_id,"input":[{"type":"text","text":task}],
        "model":options.model,"effort":options.effort,"cwd":options.cwd,"approvalPolicy":"never",
        "sandboxPolicy":options.execution.sandbox}),
    )?;
    let turn_id = started["turn"]["id"]
        .as_str()
        .ok_or("missing Codex turn ID")?
        .to_owned();
    manifest["active_turn"] = json!(turn_id);
    lifecycle::save_json(&run.join("manifest.json"), manifest)?;
    let _endpoint = Endpoint::start(run, &turn_id, server.sender.clone())?;
    eprintln!(
        "pira_team active: {} (revision {}; repair={repair})",
        manifest["run_id"].as_str().unwrap_or("?"),
        manifest["revision"]
    );
    let mut controls = create(&dir.join("controls.jsonl"))?;
    let mut pending_controls: HashMap<u64, (String, String, String, mpsc::Sender<Value>)> =
        HashMap::new();
    let (mut text, mut usage) = (None, None);
    let mut stopping: Option<(&str, Instant)> = None;
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
            terminate(server.child.id());
            return Ok(Turn {
                status: status.into(),
                text,
                usage,
                error: Some("cancellation did not settle; owned backend processes terminated (Unix process group; detached descendants excluded)".into()),
            });
        }
        let event = if let Some(value) = server.pending.pop_front() {
            Event::Protocol(value)
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
                    return Ok(Turn {
                        status: status.into(),
                        text,
                        usage,
                        error: Some(error),
                    });
                }
                return Ok(Turn {
                    status: "failed".into(),
                    text,
                    usage,
                    error: Some(error),
                });
            }
            Event::Control(request, reply) => {
                let op = request["operation"].as_str().unwrap_or("");
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
                        return Ok(Turn {
                            status: stopping.map_or("failed", |s| s.0).into(),
                            text,
                            usage,
                            error: Some(error),
                        });
                    }
                };
                writeln!(controls,"{}",json!({"request_id":id,"operation":op,"task":task,"turn_id":turn_id,"status":"sent"})).map_err(|e|e.to_string())?;
                pending_controls.insert(id, (op.into(), task.into(), gate.into(), reply));
            }
            Event::Protocol(value) => {
                if value.get("method").is_some() && value.get("id").is_some() {
                    server.send(json!({"id":value["id"],"error":{"code":-32601,"message":"unexpected request refused by coding-worker launcher"}}))?;
                    return Err("unexpected approval/tool request refused".into());
                }
                if let Some(id) = value["id"].as_u64() {
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
                                stopping =
                                    Some(("interrupted", Instant::now() + Duration::from_secs(5)));
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
                    Some("item/completed") if p["item"]["type"] == "agentMessage" => {
                        if p["item"]["phase"].is_null() || p["item"]["phase"] == "final_answer" {
                            text = p["item"]["text"].as_str().map(str::to_owned);
                        }
                    }
                    Some("thread/tokenUsage/updated") => {
                        usage = Some(normalize_usage(&p["tokenUsage"]["total"]));
                    }
                    Some("turn/completed") if p["turn"]["id"] == turn_id => {
                        let native = p["turn"]["status"].as_str().unwrap_or("failed");
                        let status = stopping.map(|s| s.0).unwrap_or(native).to_owned();
                        return Ok(Turn {
                            status,
                            text,
                            usage,
                            error: (!p["turn"]["error"].is_null())
                                .then(|| p["turn"]["error"].to_string()),
                        });
                    }
                    _ => {}
                }
            }
        }
    }
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
        let (writer, thread) = request_writer(Recorded(bytes.clone()), fault.clone(), events);
        for text in ["initialize", "initialized", "turn/start"] {
            writer.try_send(text.into()).unwrap();
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
        );
        writer.try_send("steer".into()).unwrap();
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        // The caller can still admit interruption while the transport is blocked.
        for _ in 0..PENDING_CONTROLS {
            writer.try_send("steer".into()).unwrap();
        }
        writer.try_send("interrupt".into()).unwrap();
        assert!(matches!(
            writer.try_send("overflow".into()),
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
        assert!(writer.try_send("closed".into()).is_err());
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
}
