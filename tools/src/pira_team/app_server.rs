//! Minimal native Codex transport; adapted from archived Team JSON-RPC patterns.
use crate::{Options, command, create, isolate, lifecycle, terminate};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const FRAME_LIMIT: u64 = 64 * 1024;
const RPC_LIMIT: u64 = 16 * 1024 * 1024;

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
                for connection in listener.incoming() {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = connection else {
                        break;
                    };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
                    let outcome = (|| -> Result<Value, String> {
                        let request = read_frame(&mut stream)?;
                        if request["token"].as_str() != Some(&token) {
                            return Err("invalid control capability".into());
                        }
                        let (reply, receiver) = mpsc::channel();
                        sender
                            .try_send(Event::Control(request, reply))
                            .map_err(|_| "run owner stopped")?;
                        loop {
                            match receiver.recv_timeout(Duration::from_millis(100)) {
                                Ok(value) => return Ok(value),
                                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(
                                    "run ended before acknowledgement; delivery outcome unknown"
                                        .into(),
                                ),
                                Err(_) if stopped.load(Ordering::SeqCst) => {
                                    return Err("run ended; delivery outcome unknown".into());
                                }
                                Err(_) => {}
                            }
                        }
                    })();
                    let response =
                        outcome.unwrap_or_else(|e| json!({"status":"rejected","error":e}));
                    let _ = writeln!(stream, "{response}");
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

fn read_frame(stream: &mut TcpStream) -> Result<Value, String> {
    let mut data = Vec::new();
    BufReader::new(stream)
        .take(FRAME_LIMIT + 1)
        .read_until(b'\n', &mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > FRAME_LIMIT || data.last() != Some(&b'\n') {
        return Err("invalid or oversized control frame".into());
    }
    serde_json::from_slice(&data).map_err(|_| "invalid control JSON".into())
}

pub fn control(run: &Path, operation: &str, task: &str) -> Result<Value, String> {
    let endpoint = lifecycle::read_json(&run.join("control.json"))
        .map_err(|_| "run has no active control endpoint; it may be starting or already stopped")?;
    let address: SocketAddr = endpoint["address"]
        .as_str()
        .ok_or("missing control address")?
        .parse()
        .map_err(|_| "invalid control address")?;
    if !address.ip().is_loopback() {
        return Err("control endpoint must be loopback".into());
    }
    let frame = json!({"token":endpoint["token"], "turn_id":endpoint["turn_id"], "operation":operation, "task":task});
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
    writer: Option<ChildStdin>,
    reader: Option<std::thread::JoinHandle<()>>,
    events: mpsc::Receiver<Event>,
    sender: mpsc::SyncSender<Event>,
    pending: VecDeque<Value>,
    next_id: u64,
    deadline: Instant,
    requests: File,
    fault: Arc<std::sync::Mutex<Option<String>>>,
    watchdog: Option<std::thread::JoinHandle<()>>,
    finished: Option<mpsc::Sender<()>>,
}
impl Server {
    fn new(
        options: &Options,
        run: &Path,
        dir: &Path,
        timeout: Duration,
        mode: crate::Mode,
    ) -> Result<Self, String> {
        let mut cmd = command(options, run, dir, mode);
        cmd.stdout(Stdio::piped())
            .stderr(create(&dir.join("stderr.log"))?);
        let mut log = create(&dir.join("events.jsonl"))?;
        let requests = create(&dir.join("requests.jsonl"))?;
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or("timeout exceeds clock range")?;
        isolate(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("launch Codex app-server: {e}"))?;
        let writer = child.stdin.take().ok_or("missing app-server stdin")?;
        let stdout = child.stdout.take().ok_or("missing app-server stdout")?;
        let (finished, finish_rx) = mpsc::channel();
        let pid = child.id();
        let watchdog = std::thread::spawn(move || {
            // Also cover a blocked stdin write, where the normal RPC loop cannot observe signals.
            let grace = Duration::from_secs(5);
            let mut cutoff = deadline.checked_add(grace).unwrap_or(deadline);
            loop {
                if crate::CANCELLED.load(Ordering::SeqCst) {
                    cutoff = cutoff.min(Instant::now() + grace);
                }
                let Some(remaining) = cutoff.checked_duration_since(Instant::now()) else {
                    terminate(pid);
                    break;
                };
                match finish_rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });
        let (sender, events) = mpsc::sync_channel(64);
        let fault = Arc::new(std::sync::Mutex::new(None));
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
            *reader_fault.lock().unwrap() = Some(result.unwrap_err());
        });
        Ok(Self {
            child,
            writer: Some(writer),
            reader: Some(reader),
            events,
            sender,
            pending: VecDeque::new(),
            next_id: 1,
            deadline,
            requests,
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
        let writer = self.writer.as_mut().ok_or("app-server stdin closed")?;
        writeln!(writer, "{text}")
            .and_then(|_| writer.flush())
            .map_err(|e| e.to_string())
    }
    fn send_request(&mut self, method: &str, params: Value) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"id":id,"method":method,"params":params}))?;
        Ok(id)
    }
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.send_request(method, params)?;
        loop {
            if crate::CANCELLED.load(Ordering::SeqCst) {
                return Err("interrupted during worker startup".into());
            }
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or("worker timed out during startup")?;
            match self
                .events
                .recv_timeout(remaining.min(Duration::from_millis(100)))
            {
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
    }
}

pub fn turn(
    options: &Options,
    run: &Path,
    dir: &Path,
    task: &str,
    timeout: Duration,
    manifest: &mut Value,
    repair: bool,
) -> Result<Turn, String> {
    let mode = if repair {
        crate::Mode::ReadOnly
    } else {
        options.mode
    };
    let mut server = Server::new(options, run, dir, timeout, mode)?;
    server.request("initialize", json!({"clientInfo":{"name":"pira_team","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":false}}))?;
    server.send(json!({"method":"initialized"}))?;
    let policy = fs::read_to_string(dir.join("policy.md")).map_err(|e| e.to_string())?;
    let mut params = json!({"model":options.model,"cwd":options.cwd,"sandbox":mode.sandbox(),
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
    if thread["sandbox"]["type"] != mode.sandbox_type() || thread["approvalPolicy"] != "never" {
        return Err(format!(
            "Codex did not confirm {}/approval-never permissions",
            mode.sandbox()
        ));
    }
    let thread_id = thread["thread"]["id"]
        .as_str()
        .ok_or("missing Codex thread ID")?
        .to_owned();
    manifest["thread_id"] = json!(thread_id);
    lifecycle::save_json(&run.join("manifest.json"), manifest)?;
    let sandbox = if mode == crate::Mode::Fix {
        json!({"type":"workspaceWrite","writableRoots":[options.cwd],"networkAccess":false,
            "excludeTmpdirEnvVar":true,"excludeSlashTmp":true})
    } else {
        json!({"type":"readOnly","networkAccess":false})
    };
    let started = server.request(
        "turn/start",
        json!({"threadId":thread_id,"input":[{"type":"text","text":task}],
        "model":options.model,"effort":options.effort,"cwd":options.cwd,"approvalPolicy":"never",
        "sandboxPolicy":sandbox}),
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
    let mut pending_controls: HashMap<u64, (String, mpsc::Sender<Value>)> = HashMap::new();
    let (mut text, mut usage) = (None, None);
    let mut stopping: Option<(&str, Instant)> = None;
    loop {
        if stopping.is_none()
            && (crate::CANCELLED.load(Ordering::SeqCst) || Instant::now() >= server.deadline)
        {
            let status = if crate::CANCELLED.load(Ordering::SeqCst) {
                "interrupted"
            } else {
                "timed_out"
            };
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
                text: None,
                usage,
                error: Some("cancellation did not settle; owned process tree terminated".into()),
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
                        text: None,
                        usage,
                        error: Some(error),
                    });
                }
                return Err(error);
            }
            Event::Control(request, reply) => {
                let op = request["operation"].as_str().unwrap_or("");
                let task = request["task"].as_str().unwrap_or("");
                if request["turn_id"] != turn_id
                    || stopping.is_some()
                    || !["steer", "interrupt"].contains(&op)
                    || (op == "steer" && (repair || task.trim().is_empty()))
                {
                    let _ = reply.send(json!({"status":"rejected","error":"stale/inactive turn, or steering unavailable during repair"}));
                    continue;
                }
                let (method, params) = if op == "steer" {
                    (
                        "turn/steer",
                        json!({"threadId":thread_id,"expectedTurnId":turn_id,"input":[{"type":"text","text":task}]}),
                    )
                } else {
                    (
                        "turn/interrupt",
                        json!({"threadId":thread_id,"turnId":turn_id}),
                    )
                };
                let id = server.send_request(method, params)?;
                writeln!(controls,"{}",json!({"request_id":id,"operation":op,"task":task,"turn_id":turn_id,"status":"sent"})).map_err(|e|e.to_string())?;
                pending_controls.insert(id, (op.into(), reply));
            }
            Event::Protocol(value) => {
                if value.get("method").is_some() && value.get("id").is_some() {
                    server.send(json!({"id":value["id"],"error":{"code":-32601,"message":"unexpected request refused by read-only launcher"}}))?;
                    return Err("unexpected approval/tool request refused".into());
                }
                if let Some(id) = value["id"].as_u64() {
                    if let Some((op, reply)) = pending_controls.remove(&id) {
                        let response = if let Some(error) = value.get("error") {
                            json!({"status":"rejected","error":error.to_string()})
                        } else if value.get("result").is_none() {
                            json!({"status":"rejected","error":"missing control acknowledgement"})
                        } else if op == "steer" && value["result"]["turnId"] != turn_id {
                            json!({"status":"rejected","error":"steer acknowledgement has wrong turn ID"})
                        } else {
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
