use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::language::Language;
use crate::model::Symbol;

use super::protocol::{
    MAX_CALL_ITEMS, MAX_CALL_RANGES, MAX_TYPE_ITEMS, PositionEncoding, SourcePositions,
    bounded_text, file_uri, language_id, parse_call_items, parse_calls, parse_document_symbols,
    parse_hover, parse_locations, parse_type_items, parse_type_relations,
};
use super::{LspCall, LspConfig, LspHover, LspLocation, LspTypeItem};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_MESSAGES_PER_REQUEST: usize = 10_000;
const MAX_PENDING_MESSAGES: usize = 256;
const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;
const MAX_SERVER_STDERR_BYTES: usize = 16 * 1024;
const LSP_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const LSP_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

type WriteRequest = (Vec<u8>, mpsc::SyncSender<Result<(), String>>);
type InboundMessage = (Result<Value, String>, usize);

#[derive(Default)]
struct InboundBudget {
    bytes: AtomicUsize,
    overflow: AtomicBool,
}

impl InboundBudget {
    fn enqueue(&self, sender: &mpsc::SyncSender<InboundMessage>, message: InboundMessage) -> bool {
        let size = message.1;
        if self
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(size)
                    .filter(|total| *total <= MAX_PENDING_BYTES)
            })
            .is_err()
        {
            self.overflow.store(true, Ordering::Release);
            return false;
        }
        match sender.try_send(message) {
            Ok(()) => true,
            Err(error) => {
                self.bytes.fetch_sub(size, Ordering::AcqRel);
                if matches!(error, mpsc::TrySendError::Full(_)) {
                    self.overflow.store(true, Ordering::Release);
                }
                false
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ServerCapabilities {
    document_symbols: bool,
    definition: bool,
    implementation: bool,
    type_definition: bool,
    references: bool,
    hover: bool,
    call_hierarchy: bool,
    type_hierarchy: bool,
}

struct DocumentPosition<'a> {
    path: &'a Path,
    language: Language,
    source: &'a str,
    row: usize,
    byte_column: usize,
}

pub(super) struct LspClient {
    process: ServerProcess,
    input: Option<mpsc::Sender<WriteRequest>>,
    output: mpsc::Receiver<InboundMessage>,
    inbound: Arc<InboundBudget>,
    stderr: Arc<Mutex<Vec<u8>>>,
    next_id: u64,
    encoding: PositionEncoding,
    capabilities: ServerCapabilities,
    settings: Option<Value>,
    retain_documents: bool,
    open_documents: BTreeMap<String, Vec<usize>>,
    unusable: bool,
    request_timeout: Duration,
}

impl LspClient {
    pub(super) fn start(config: &LspConfig, retain_documents: bool) -> Result<Self, String> {
        if crate::cancellation::requested() {
            return Err("LSP operation cancelled".into());
        }
        let mut command = Command::new(&config.executable);
        command
            .args(&config.arguments)
            .current_dir(&config.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_process_group(&mut command);
        let child = command.spawn().map_err(|error| {
            format!(
                "cannot start LSP server {}: {error}",
                config.executable.display()
            )
        })?;
        // Install ownership before any fallible pipe/thread/job setup.
        let mut process = ServerProcess::new(child);
        #[cfg(windows)]
        {
            process.job = create_kill_job(&process.child)
                .map_err(|e| format!("cannot isolate LSP server process tree: {e}"))?;
        }
        let input = process
            .child
            .stdin
            .take()
            .ok_or_else(|| "LSP server stdin was not available".to_string())?;
        let output = process
            .child
            .stdout
            .take()
            .ok_or_else(|| "LSP server stdout was not available".to_string())?;
        let stderr = process
            .child
            .stderr
            .take()
            .ok_or_else(|| "LSP server stderr was not available".to_string())?;
        let input = InterruptiblePipe::new(input, Arc::clone(&process.stop))?;
        let output = InterruptiblePipe::new(output, Arc::clone(&process.stop))?;
        let stderr = InterruptiblePipe::new(stderr, Arc::clone(&process.stop))?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_thread = Arc::clone(&captured);
        process.spawn("stderr", move || {
            let mut stderr = stderr;
            let mut chunk = [0u8; 8 * 1024];
            while let Ok(count) = stderr.read(&mut chunk) {
                if count == 0 {
                    break;
                }
                if let Ok(mut bytes) = captured_for_thread.lock() {
                    let remaining = MAX_SERVER_STDERR_BYTES.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&chunk[..count.min(remaining)]);
                }
            }
        })?;
        let (input_sender, input_receiver) = mpsc::channel::<WriteRequest>();
        process.spawn("stdin", move || {
            let mut input = BufWriter::new(input);
            for (bytes, reply) in input_receiver {
                let result = input
                    .write_all(&bytes)
                    .and_then(|_| input.flush())
                    .map_err(|error| error.to_string());
                let failed = result.is_err();
                let _ = reply.send(result);
                if failed {
                    break;
                }
            }
        })?;
        let (output_sender, output_receiver) = mpsc::sync_channel(MAX_PENDING_MESSAGES);
        let inbound = Arc::new(InboundBudget::default());
        let reader_budget = Arc::clone(&inbound);
        process.spawn("stdout", move || {
            let mut output = BufReader::new(output);
            loop {
                let (message, size) = match read_message(&mut output) {
                    Ok((message, size)) => (Ok(message), size),
                    Err(error) => (Err(error), 0),
                };
                let failed = message.is_err();
                // Never block the reader while the peer is waiting for us to finish a write.
                if !reader_budget.enqueue(&output_sender, (message, size)) || failed {
                    break;
                }
            }
        })?;
        let mut client = Self {
            process,
            input: Some(input_sender),
            output: output_receiver,
            inbound,
            stderr: captured,
            next_id: 1,
            encoding: PositionEncoding::Utf16,
            capabilities: ServerCapabilities::default(),
            settings: config.settings.clone(),
            retain_documents,
            open_documents: BTreeMap::new(),
            unusable: false,
            request_timeout: LSP_REQUEST_TIMEOUT,
        };
        let root_uri = file_uri(&config.root)?;
        let root_name = config
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace");
        let mut initialize_params = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "pira_nav", "version": env!("CARGO_PKG_VERSION")},
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": root_uri, "name": root_name}],
            "capabilities": {
                "general": {"positionEncodings": ["utf-8", "utf-16", "utf-32"]},
                "textDocument": {
                    "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
                    "definition": {"linkSupport": true},
                    "implementation": {"linkSupport": true},
                    "typeDefinition": {"linkSupport": true},
                    "references": {},
                    "hover": {"contentFormat": ["markdown", "plaintext"]},
                    "callHierarchy": {"dynamicRegistration": false},
                    "typeHierarchy": {"dynamicRegistration": false}
                },
                "workspace": {"workspaceFolders": true, "applyEdit": false}
            }
        });
        if let Some(options) = &config.initialization_options {
            initialize_params["initializationOptions"] = options.clone();
        }
        let initialized = client.request("initialize", initialize_params)?;
        let capabilities = initialized
            .get("capabilities")
            .and_then(Value::as_object)
            .ok_or_else(|| "LSP initialize result omitted capabilities".to_string())?;
        client.capabilities = ServerCapabilities {
            document_symbols: provider_enabled(capabilities.get("documentSymbolProvider")),
            definition: provider_enabled(capabilities.get("definitionProvider")),
            implementation: provider_enabled(capabilities.get("implementationProvider")),
            type_definition: provider_enabled(capabilities.get("typeDefinitionProvider")),
            references: provider_enabled(capabilities.get("referencesProvider")),
            hover: provider_enabled(capabilities.get("hoverProvider")),
            call_hierarchy: provider_enabled(capabilities.get("callHierarchyProvider")),
            type_hierarchy: provider_enabled(capabilities.get("typeHierarchyProvider")),
        };
        client.encoding = match capabilities
            .get("positionEncoding")
            .and_then(Value::as_str)
            .unwrap_or("utf-16")
        {
            "utf-8" => PositionEncoding::Utf8,
            "utf-16" => PositionEncoding::Utf16,
            "utf-32" => PositionEncoding::Utf32,
            other => {
                return Err(format!(
                    "unsupported LSP position encoding: {}",
                    bounded_text(other, 128)
                ));
            }
        };
        client.notify("initialized", json!({}))?;
        if let Some(settings) = client.settings.clone() {
            client.notify(
                "workspace/didChangeConfiguration",
                json!({"settings": settings}),
            )?;
        }
        Ok(client)
    }

    pub(super) fn document_symbols(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
    ) -> Result<Vec<Symbol>, String> {
        self.require_capability(self.capabilities.document_symbols, "document symbols")?;
        let uri = self.open_document(path, language, source)?;
        let result = self.request(
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": uri}}),
        );
        self.close_document(&uri);
        parse_document_symbols(&result?, &uri, source, language, self.encoding)
    }

    pub(super) fn require_definitions(&self) -> Result<(), String> {
        self.require_capability(
            self.capabilities.definition,
            "definition (required for imports; configure a server with definitionProvider)",
        )
    }

    pub(super) fn definition(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
    ) -> Result<Vec<LspLocation>, String> {
        self.require_capability(self.capabilities.definition, "definition")?;
        let result = self.position_request(
            "textDocument/definition",
            DocumentPosition {
                path,
                language,
                source,
                row,
                byte_column,
            },
            None,
        )?;
        parse_locations(&result, true, self.encoding)
    }

    pub(super) fn implementation(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
    ) -> Result<Vec<LspLocation>, String> {
        self.require_capability(self.capabilities.implementation, "implementation")?;
        let result = self.position_request(
            "textDocument/implementation",
            DocumentPosition {
                path,
                language,
                source,
                row,
                byte_column,
            },
            None,
        )?;
        parse_locations(&result, true, self.encoding)
    }

    pub(super) fn type_definition(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
    ) -> Result<Vec<LspLocation>, String> {
        self.require_capability(self.capabilities.type_definition, "type definition")?;
        let result = self.position_request(
            "textDocument/typeDefinition",
            DocumentPosition {
                path,
                language,
                source,
                row,
                byte_column,
            },
            None,
        )?;
        parse_locations(&result, true, self.encoding)
    }

    pub(super) fn references(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
        include_declaration: bool,
    ) -> Result<Vec<LspLocation>, String> {
        self.require_capability(self.capabilities.references, "references")?;
        let result = self.position_request(
            "textDocument/references",
            DocumentPosition {
                path,
                language,
                source,
                row,
                byte_column,
            },
            Some(json!({"includeDeclaration": include_declaration})),
        )?;
        parse_locations(&result, false, self.encoding)
    }

    pub(super) fn hover(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
    ) -> Result<Option<LspHover>, String> {
        self.require_capability(self.capabilities.hover, "hover")?;
        let result = self.position_request(
            "textDocument/hover",
            DocumentPosition {
                path,
                language,
                source,
                row,
                byte_column,
            },
            None,
        )?;
        parse_hover(&result, self.encoding)
    }

    pub(super) fn calls(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
        incoming: bool,
    ) -> Result<Vec<LspCall>, String> {
        self.require_capability(self.capabilities.call_hierarchy, "call hierarchy")?;
        let uri = self.open_document(path, language, source)?;
        let position = match self.document_position(&uri, source, row, byte_column) {
            Ok(position) => position,
            Err(error) => {
                self.close_document(&uri);
                return Err(error);
            }
        };
        let prepared = self.request(
            "textDocument/prepareCallHierarchy",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.line, "character": position.character}
            }),
        );
        let result = (|| {
            let items = parse_call_items(&prepared?)?;
            let method = if incoming {
                "callHierarchy/incomingCalls"
            } else {
                "callHierarchy/outgoingCalls"
            };
            let mut calls = Vec::new();
            let mut total_ranges = 0usize;
            for item in items {
                let value = self.request(method, json!({"item": item}))?;
                let caller_uri = item["uri"].as_str().expect("validated call item URI");
                let next = parse_calls(&value, incoming, caller_uri, self.encoding)?;
                if calls.len().saturating_add(next.len()) > MAX_CALL_ITEMS {
                    return Err("LSP call hierarchy result exceeds the safety limit".into());
                }
                total_ranges = total_ranges.saturating_add(
                    next.iter()
                        .map(|call| call.call_ranges.len())
                        .sum::<usize>(),
                );
                if total_ranges > MAX_CALL_RANGES {
                    return Err("LSP call hierarchy ranges exceed the safety limit".into());
                }
                calls.extend(next);
            }
            Ok(calls)
        })();
        self.close_document(&uri);
        result
    }

    pub(super) fn type_hierarchy(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
        supertypes: bool,
    ) -> Result<Vec<LspTypeItem>, String> {
        self.require_capability(self.capabilities.type_hierarchy, "type hierarchy")?;
        let uri = self.open_document(path, language, source)?;
        let position = match self.document_position(&uri, source, row, byte_column) {
            Ok(position) => position,
            Err(error) => {
                self.close_document(&uri);
                return Err(error);
            }
        };
        let prepared = self.request(
            "textDocument/prepareTypeHierarchy",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.line, "character": position.character}
            }),
        );
        let result = (|| {
            let items = parse_type_items(&prepared?)?;
            let method = if supertypes {
                "typeHierarchy/supertypes"
            } else {
                "typeHierarchy/subtypes"
            };
            let mut relations = Vec::new();
            for item in items {
                let value = self.request(method, json!({"item": item}))?;
                let next = parse_type_relations(&value, self.encoding)?;
                if relations.len().saturating_add(next.len()) > MAX_TYPE_ITEMS {
                    return Err("LSP type hierarchy result exceeds the safety limit".into());
                }
                relations.extend(next);
            }
            Ok(relations)
        })();
        self.close_document(&uri);
        result
    }

    fn require_capability(&self, available: bool, name: &str) -> Result<(), String> {
        available
            .then_some(())
            .ok_or_else(|| format!("LSP server does not advertise {name}"))
    }

    fn open_document(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
    ) -> Result<String, String> {
        let uri = file_uri(path)?;
        if self.retain_documents && self.open_documents.contains_key(&uri) {
            return Ok(uri);
        }
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id(language),
                    "version": 1,
                    "text": source
                }
            }),
        )?;
        self.open_documents
            .insert(uri.clone(), SourcePositions::index(source));
        Ok(uri)
    }

    fn close_document(&mut self, uri: &str) {
        if self.retain_documents {
            return;
        }
        self.open_documents.remove(uri);
        let _ = self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        );
    }

    fn position_request(
        &mut self,
        method: &str,
        target: DocumentPosition<'_>,
        context: Option<Value>,
    ) -> Result<Value, String> {
        let uri = self.open_document(target.path, target.language, target.source)?;
        let position =
            match self.document_position(&uri, target.source, target.row, target.byte_column) {
                Ok(position) => position,
                Err(error) => {
                    self.close_document(&uri);
                    return Err(error);
                }
            };
        let mut params = json!({
            "textDocument": {"uri": uri},
            "position": {"line": position.line, "character": position.character}
        });
        if let Some(context) = context {
            params["context"] = context;
        }
        let result = self.request(method, params);
        self.close_document(&uri);
        result
    }

    fn document_position(
        &self,
        uri: &str,
        source: &str,
        row: usize,
        byte_column: usize,
    ) -> Result<super::protocol::LspPosition, String> {
        let line_starts = self
            .open_documents
            .get(uri)
            .ok_or_else(|| "LSP document position requested before didOpen".to_string())?;
        SourcePositions::with_index(source, self.encoding, line_starts)
            .lsp_position(row, byte_column)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))?;
        let deadline = Instant::now() + self.request_timeout;
        for _ in 0..MAX_MESSAGES_PER_REQUEST {
            let message = self.read(deadline)?;
            if message.get("id").and_then(Value::as_u64) == Some(id)
                && message.get("method").is_none()
            {
                if let Some(error) = message.get("error") {
                    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                    let text = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unspecified server error");
                    return Err(format!(
                        "LSP {method} failed ({code}): {}",
                        bounded_text(text, 2 * 1024)
                    ));
                }
                return message
                    .get("result")
                    .cloned()
                    .ok_or_else(|| format!("LSP {method} response omitted result"));
            }
            if message.get("method").is_some() && message.get("id").is_some() {
                self.respond_to_server_request(&message)?;
            }
        }
        Err(format!(
            "LSP {method} produced too many unrelated messages without a response"
        ))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn respond_to_server_request(&mut self, message: &Value) -> Result<(), String> {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if method == "workspace/configuration" {
            let payload = configuration_response(message, self.settings.as_ref())?;
            return self.send_payload(payload);
        }
        let id = message
            .get("id")
            .cloned()
            .ok_or("LSP server request omitted id")?;
        let response = match method {
            "workspace/applyEdit" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"applied": false, "failureReason": "pira_nav is read-only"}
            }),
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create" => {
                json!({"jsonrpc": "2.0", "id": id, "result": null})
            }
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "unsupported by read-only pira_nav client"}
            }),
        };
        self.send(&response)
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        self.check_inbound_budget()?;
        if self.unusable {
            return Err("LSP client is unavailable after a previous I/O failure".into());
        }
        let mut output = CappedBytes::default();
        serde_json::to_writer(&mut output, message)
            .map_err(|error| format!("cannot encode LSP message: {error}"))?;
        self.send_payload(output.0)
    }

    fn send_payload(&mut self, payload: Vec<u8>) -> Result<(), String> {
        self.check_inbound_budget()?;
        if self.unusable {
            return Err("LSP client is unavailable after a previous I/O failure".into());
        }
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err(format!(
                "LSP message exceeds the {} MiB safety limit",
                MAX_MESSAGE_BYTES / (1024 * 1024)
            ));
        }
        let mut framed = format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
        framed.extend_from_slice(&payload);
        let (reply, result) = mpsc::sync_channel(1);
        self.input
            .as_ref()
            .ok_or_else(|| "LSP writer is closed".to_string())?
            .send((framed, reply))
            .map_err(|_| "LSP writer stopped unexpectedly".to_string())?;
        let written = receive(
            &result,
            Instant::now() + LSP_WRITE_TIMEOUT,
            &self.process.stop,
        );
        self.check_inbound_budget()?;
        match written {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(self.io_error("write", io::Error::other(error))),
            Err(_) => {
                self.unusable = true;
                self.terminate_tree();
                Err(format!(
                    "LSP write timed out after {}s{}",
                    LSP_WRITE_TIMEOUT.as_secs(),
                    self.stderr_excerpt()
                ))
            }
        }
    }

    fn read(&mut self, deadline: Instant) -> Result<Value, String> {
        self.check_inbound_budget()?;
        let received = receive(&self.output, deadline, &self.process.stop);
        if let Ok((_, size)) = &received {
            self.inbound.bytes.fetch_sub(*size, Ordering::AcqRel);
        }
        self.check_inbound_budget()?;
        match received.map(|(message, _)| message) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(self.io_error("read", io::Error::other(error))),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(self.io_error("read", io::Error::from(io::ErrorKind::UnexpectedEof)))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.unusable = true;
                self.terminate_tree();
                Err(format!(
                    "LSP request timed out after {}s{}",
                    self.request_timeout.as_secs_f64(),
                    self.stderr_excerpt()
                ))
            }
        }
    }

    fn check_inbound_budget(&mut self) -> Result<(), String> {
        if crate::cancellation::requested() || self.process.stop.load(Ordering::Acquire) {
            self.unusable = true;
            self.terminate_tree();
            return Err("LSP operation cancelled".into());
        }
        if self.inbound.overflow.load(Ordering::Acquire) {
            self.unusable = true;
            self.terminate_tree();
            return Err("LSP unsolicited message queue exceeds the safety limit".into());
        }
        Ok(())
    }

    fn terminate_tree(&mut self) {
        self.process.terminate();
    }

    fn io_error(&mut self, operation: &str, error: io::Error) -> String {
        self.unusable = true;
        self.terminate_tree();
        let status = self
            .process
            .child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| format!("; server exited with {status}"))
            .unwrap_or_default();
        let stderr = self.stderr_excerpt();
        format!("cannot {operation} LSP message: {error}{status}{stderr}")
    }
}

fn receive<T>(
    receiver: &mpsc::Receiver<T>,
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<T, mpsc::RecvTimeoutError> {
    loop {
        if stop.load(Ordering::Acquire) || crate::cancellation::requested() {
            return Err(mpsc::RecvTimeoutError::Disconnected);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining.min(Duration::from_millis(25))) {
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
            result => return result,
        }
    }
}

fn read_message(output: &mut impl BufRead) -> Result<(Value, usize), String> {
    let mut content_length = None;
    let mut header_bytes = 0usize;
    loop {
        let mut line = Vec::new();
        let allowance = MAX_HEADER_LINE_BYTES.min(MAX_HEADER_BYTES - header_bytes);
        let count = output
            .by_ref()
            .take(allowance as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("cannot read LSP message: {error}"))?;
        if count == 0 {
            return Err("cannot read LSP message: unexpected end of file".into());
        }
        header_bytes = header_bytes.saturating_add(count);
        if line.len() > MAX_HEADER_LINE_BYTES || header_bytes > MAX_HEADER_BYTES {
            return Err("LSP response headers exceed the safety limit".into());
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let text = std::str::from_utf8(&line)
            .map_err(|_| "LSP response header is not valid UTF-8".to_string())?;
        let (name, value) = text
            .split_once(':')
            .ok_or_else(|| "malformed LSP response header".to_string())?;
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err("duplicate LSP Content-Length header".into());
            }
            let length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "invalid LSP Content-Length header".to_string())?;
            if length > MAX_MESSAGE_BYTES {
                return Err(format!(
                    "LSP response exceeds the {} MiB safety limit",
                    MAX_MESSAGE_BYTES / (1024 * 1024)
                ));
            }
            content_length = Some(length);
        }
    }
    let length = content_length.ok_or_else(|| "LSP response omitted Content-Length".to_string())?;
    let mut payload = vec![0u8; length];
    output
        .read_exact(&mut payload)
        .map_err(|error| format!("cannot read LSP message: {error}"))?;
    let value: Value = serde_json::from_slice(&payload)
        .map_err(|error| format!("invalid JSON from LSP server: {error}"))?;
    if !value.is_object() {
        return Err("LSP message must be a JSON object".into());
    }
    Ok((value, length))
}

impl LspClient {
    fn stderr_excerpt(&self) -> String {
        let Ok(bytes) = self.stderr.lock() else {
            return String::new();
        };
        if bytes.is_empty() {
            return String::new();
        }
        let text = String::from_utf8_lossy(&bytes);
        format!("; server stderr: {}", bounded_text(&text, 2 * 1024))
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Best-effort shutdown shares one short deadline, not one timeout per
        // document. Never wait for a peer to acknowledge shutdown during Drop.
        if !self.unusable && !crate::cancellation::requested() {
            if let Some(input) = self.input.as_ref() {
                if let Ok(payload) = serde_json::to_vec(&json!({
                    "jsonrpc":"2.0", "id":self.next_id, "method":"shutdown", "params":null
                })) {
                    let mut frame =
                        format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
                    frame.extend(payload);
                    let exit = b"{\"jsonrpc\":\"2.0\",\"method\":\"exit\",\"params\":null}";
                    frame.extend(format!("Content-Length: {}\r\n\r\n", exit.len()).bytes());
                    frame.extend(exit);
                    let (reply, result) = mpsc::sync_channel(1);
                    let _ = input.send((frame, reply));
                    let _ = result.recv_timeout(Duration::from_millis(50));
                }
            }
        }
        self.input.take();
        // ServerProcess owns termination, reaping, and interruptible pipe joins.
    }
}

struct ServerProcess {
    child: Child,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    terminated: bool,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}
impl ServerProcess {
    fn new(child: Child) -> Self {
        #[cfg(test)]
        STARTED_CHILD.with(|pid| pid.set(Some(child.id())));
        Self {
            child,
            stop: Arc::new(AtomicBool::new(false)),
            threads: Vec::new(),
            terminated: false,
            #[cfg(windows)]
            job: std::ptr::null_mut(),
        }
    }
    fn spawn(&mut self, name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
        #[cfg(test)]
        if SPAWN_FAILURE.with(|at| match at.get() {
            Some(0) => true,
            Some(n) => {
                at.set(Some(n - 1));
                false
            }
            None => false,
        }) {
            return Err("injected LSP thread creation failure".into());
        }
        let handle = thread::Builder::new()
            .name(format!("nav-lsp-{name}"))
            .spawn(work)
            .map_err(|e| format!("cannot start LSP {name} thread: {e}"))?;
        self.threads.push(handle);
        Ok(())
    }
    fn terminate(&mut self) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        self.stop.store(true, Ordering::Release);
        // The leader is never reaped until its owned group has been terminated,
        // so the PID/group number cannot be reused by an unrelated process.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        unsafe {
            if !self.job.is_null() {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
            } else {
                let _ = self.child.kill();
            }
        }
        // Also target the still-owned child handle if a peer moved its group or
        // a platform group/job termination failed. This never targets a reaped PID.
        let _ = self.child.kill();
    }
}
impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.terminate();
        let deadline = Instant::now() + Duration::from_millis(500);
        // A stuck kernel/foreign pipe holder must not turn cleanup into an
        // unbounded wait. Never send a second signal after reaping the leader.
        while Instant::now() < deadline {
            #[cfg(windows)]
            for handle in &self.threads {
                use std::os::windows::io::AsRawHandle;
                // SAFETY: handle is owned and live; only this thread's pending
                // synchronous I/O is cancelled, never another process's I/O.
                unsafe {
                    CancelSynchronousIo(handle.as_raw_handle());
                }
            }
            let reaped = self.child.try_wait().is_ok_and(|s| s.is_some());
            if reaped && self.threads.iter().all(JoinHandle::is_finished) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        for handle in self.threads.drain(..) {
            if handle.is_finished() {
                let _ = handle.join();
            }
            // PIRA: a platform I/O operation stuck beyond 500 ms is detached;
            // the CLI exits without chasing/killing an unowned pipe holder.
        }
        #[cfg(windows)]
        if !self.job.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.job);
            }
        }
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CancelSynchronousIo(thread: *mut std::ffi::c_void) -> i32;
}

#[cfg(test)]
thread_local! {
    static SPAWN_FAILURE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static STARTED_CHILD: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

struct InterruptiblePipe<T> {
    pipe: T,
    stop: Arc<AtomicBool>,
}
impl<T> InterruptiblePipe<T> {
    #[cfg(unix)]
    fn new(pipe: T, stop: Arc<AtomicBool>) -> Result<Self, String>
    where
        T: std::os::fd::AsRawFd,
    {
        let fd = pipe.as_raw_fd();
        // SAFETY: fd is a live owned pipe; preserve existing status flags.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(format!(
                "cannot make LSP pipe interruptible: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(Self { pipe, stop })
    }
    #[cfg(not(unix))]
    fn new(pipe: T, stop: Arc<AtomicBool>) -> Result<Self, String> {
        Ok(Self { pipe, stop })
    }
    fn check(&self) -> io::Result<()> {
        if self.stop.load(Ordering::Acquire) || crate::cancellation::requested() {
            Err(io::Error::other("LSP operation cancelled"))
        } else {
            Ok(())
        }
    }
}
#[cfg(unix)]
impl<T: std::os::fd::AsRawFd> InterruptiblePipe<T> {
    fn ready(&self, events: i16) -> io::Result<()> {
        self.check()?;
        let mut fd = libc::pollfd {
            fd: self.pipe.as_raw_fd(),
            events,
            revents: 0,
        };
        // SAFETY: poll borrows one initialized descriptor for at most 25 ms.
        if unsafe { libc::poll(&mut fd, 1, 25) } < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        self.check()
    }
}
#[cfg(unix)]
impl<T: Read + std::os::fd::AsRawFd> Read for InterruptiblePipe<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.pipe.read(bytes) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    self.ready(libc::POLLIN)?
                }
                result => return result,
            }
        }
    }
}
#[cfg(unix)]
impl<T: Write + std::os::fd::AsRawFd> Write for InterruptiblePipe<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.pipe.write(bytes) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    self.ready(libc::POLLOUT)?
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.check()?;
        self.pipe.flush()
    }
}
#[cfg(not(unix))]
impl<T: Read> Read for InterruptiblePipe<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.pipe.read(bytes)
    }
}
#[cfg(not(unix))]
impl<T: Write> Write for InterruptiblePipe<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.check()?;
        self.pipe.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.check()?;
        self.pipe.flush()
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(windows)]
fn create_kill_job(child: &Child) -> Result<windows_sys::Win32::Foundation::HANDLE, String> {
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    // SAFETY: APIs receive initialized values of documented sizes; failure paths close `job`.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error().to_string());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            let error = io::Error::last_os_error().to_string();
            CloseHandle(job);
            return Err(error);
        }
        if AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
            let error = io::Error::last_os_error().to_string();
            CloseHandle(job);
            return Err(error);
        }
        Ok(job)
    }
}

fn configuration_section<'a>(settings: Option<&'a Value>, section: &str) -> Option<&'a Value> {
    let mut value = settings?;
    if section.is_empty() {
        return Some(value);
    }
    for component in section.split('.') {
        value = value.get(component)?;
    }
    Some(value)
}

const MAX_CONFIGURATION_ITEMS: usize = 256;

#[derive(Default)]
struct CappedBytes(Vec<u8>);
impl Write for CappedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_MESSAGE_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other(
                "LSP message exceeds the 16 MiB safety limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct SizeBudget(usize);
impl Write for SizeBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| io::Error::other("LSP configuration exceeds the 16 MiB safety limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn configuration_response(message: &Value, settings: Option<&Value>) -> Result<Vec<u8>, String> {
    let items = message.pointer("/params/items").and_then(Value::as_array);
    let items = items.map(Vec::as_slice).unwrap_or(&[]);
    if items.len() > MAX_CONFIGURATION_ITEMS {
        return Err(format!(
            "LSP configuration exceeds {MAX_CONFIGURATION_ITEMS} items"
        ));
    }
    let id = message.get("id").ok_or("LSP server request omitted id")?;
    let values = items
        .iter()
        .map(|item| match item.get("section") {
            None | Some(Value::Null) => settings.unwrap_or(&Value::Null),
            Some(Value::String(section)) => {
                configuration_section(settings, section).unwrap_or(&Value::Null)
            }
            Some(_) => &Value::Null,
        })
        .collect::<Vec<_>>();
    // Compute the aggregate serialized budget using borrowed values and no reply
    // allocation, then encode. No repeated settings clones, even on rejection.
    let mut budget = SizeBudget(MAX_MESSAGE_BYTES);
    let encode = |out: &mut dyn Write| -> Result<(), String> {
        out.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":")
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut *out, id).map_err(|e| e.to_string())?;
        out.write_all(b",\"result\":").map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut *out, &values).map_err(|e| e.to_string())?;
        out.write_all(b"}").map_err(|e| e.to_string())
    };
    encode(&mut budget)?;
    let mut output = CappedBytes::default();
    encode(&mut output)?;
    Ok(output.0)
}

fn provider_enabled(value: Option<&Value>) -> bool {
    value.is_some_and(|provider| provider == true || provider.is_object())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::{
        InboundBudget, LspClient, LspConfig, MAX_HEADER_BYTES, MAX_HEADER_LINE_BYTES,
        MAX_PENDING_BYTES, MAX_PENDING_MESSAGES, Value, configuration_section, mpsc, read_message,
    };

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn configuration_item_limit_rejects_before_selecting_settings() {
        let message =
            json!({"id":1, "params":{"items":vec![json!({}); super::MAX_CONFIGURATION_ITEMS + 1]}});
        assert!(
            super::configuration_response(&message, None)
                .unwrap_err()
                .contains("items")
        );
    }

    #[test]
    fn configuration_aggregate_limit_precedes_response_allocation() {
        let message = json!({"id":1, "params":{"items":vec![json!({}); 256]}});
        let settings = json!({"value":"x".repeat(70_000)});
        assert!(
            super::configuration_response(&message, Some(&settings))
                .unwrap_err()
                .contains("16 MiB")
        );
        let message = json!({"id":"a", "params":{"items":[{}, {"section":"value"}, {"section":"missing"}, {"section":42}]}});
        let payload = super::configuration_response(&message, Some(&json!({"value":"é"}))).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&payload).unwrap(),
            json!({"jsonrpc":"2.0", "id":"a", "result":[{"value":"é"}, "é", null, null]})
        );
    }

    #[cfg(unix)]
    #[test]
    fn each_thread_creation_failure_reaps_the_immediately_guarded_child() {
        for index in 0..3 {
            let config = LspConfig::new(
                PathBuf::from("/usr/bin/env"),
                vec![
                    "python3".into(),
                    "-c".into(),
                    "import time; time.sleep(1)".into(),
                ],
                std::env::temp_dir().canonicalize().unwrap(),
                None,
                None,
            )
            .unwrap();
            super::SPAWN_FAILURE.with(|at| at.set(Some(index)));
            let start = Instant::now();
            let result = LspClient::start(&config, false);
            super::SPAWN_FAILURE.with(|at| at.set(None));
            assert!(result.err().unwrap().contains("injected LSP thread"));
            assert!(start.elapsed() < Duration::from_secs(2));
            let pid = super::STARTED_CHILD.with(|pid| pid.get().unwrap());
            // Query only this test's already reaped child; no signals or PID scans.
            assert_eq!(
                unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        }
    }

    #[test]
    fn injected_cancellation_interrupts_pending_channel_waits() {
        let (_sender, receiver) = mpsc::channel::<()>();
        let stop = std::sync::atomic::AtomicBool::new(true);
        let started = Instant::now();
        assert!(
            super::receive(&receiver, Instant::now() + Duration::from_secs(120), &stop).is_err()
        );
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn header_limits_apply_before_reading_past_the_sentinel() {
        let bytes = vec![b'x'; MAX_HEADER_LINE_BYTES + 100];
        let mut reader = std::io::Cursor::new(bytes);
        assert!(
            read_message(&mut reader)
                .unwrap_err()
                .contains("headers exceed")
        );
        assert_eq!(reader.position() as usize, MAX_HEADER_LINE_BYTES + 1);

        let line = format!("X:{}\n", "x".repeat(MAX_HEADER_LINE_BYTES - 3));
        let mut reader = std::io::Cursor::new(line.repeat(MAX_HEADER_BYTES / line.len() + 1));
        assert!(
            read_message(&mut reader)
                .unwrap_err()
                .contains("headers exceed")
        );
        assert_eq!(reader.position() as usize, MAX_HEADER_BYTES + 1);
        let mut reader = std::io::Cursor::new(b"Content-Length: 2\r\n\r\n{}");
        assert_eq!(read_message(&mut reader).unwrap(), (json!({}), 2));
    }

    #[test]
    fn unsolicited_queue_fails_without_blocking_at_item_limit() {
        let budget = InboundBudget::default();
        let (sender, receiver) = mpsc::sync_channel(MAX_PENDING_MESSAGES);
        for _ in 0..MAX_PENDING_MESSAGES {
            assert!(budget.enqueue(&sender, (Ok(Value::Null), 4)));
        }
        assert!(!budget.enqueue(&sender, (Ok(Value::Null), 4)));
        assert!(budget.overflow.load(Ordering::Acquire));
        assert_eq!(
            budget.bytes.load(Ordering::Acquire),
            MAX_PENDING_MESSAGES * 4
        );
        assert_eq!(receiver.try_iter().count(), MAX_PENDING_MESSAGES);
    }

    #[test]
    fn unsolicited_queue_respects_byte_budget_and_released_capacity() {
        let budget = InboundBudget::default();
        let (sender, receiver) = mpsc::sync_channel(4);
        // Exercise accounting without allocating a maximum-size frame.
        assert!(budget.enqueue(&sender, (Ok(Value::Null), MAX_PENDING_BYTES)));
        let (_, size) = receiver.recv().unwrap();
        budget.bytes.fetch_sub(size, Ordering::AcqRel);
        assert!(budget.enqueue(&sender, (Ok(Value::Null), MAX_PENDING_BYTES)));
        assert!(!budget.enqueue(&sender, (Ok(Value::Null), 1)));
        assert!(budget.overflow.load(Ordering::Acquire));
        assert_eq!(budget.bytes.load(Ordering::Acquire), MAX_PENDING_BYTES);
    }

    #[test]
    fn configuration_sections_support_whole_and_dotted_settings() {
        let settings = json!({"python": {"analysis": {"level": "strict"}}});
        assert_eq!(configuration_section(Some(&settings), ""), Some(&settings));
        assert_eq!(
            configuration_section(Some(&settings), "python.analysis"),
            Some(&json!({"level": "strict"}))
        );
        assert_eq!(
            configuration_section(Some(&settings), "python.missing"),
            None
        );
    }

    #[cfg(unix)]
    fn stalled_client() -> (PathBuf, LspClient) {
        let directory = std::env::temp_dir().join(format!(
            "pira-nav-lsp-timeout-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let server = directory.join("server.py");
        fs::write(
            &server,
            r#"#!/usr/bin/env python3
import json, sys, time, signal
signal.alarm(5)
def read():
    n = 0
    while True:
        line = sys.stdin.buffer.readline()
        if line in (b'\r\n', b'\n', b''): break
        if line.lower().startswith(b'content-length:'): n = int(line.split(b':', 1)[1])
    return json.loads(sys.stdin.buffer.read(n))
def send(v):
    b = json.dumps(v).encode()
    sys.stdout.buffer.write(b'Content-Length: ' + str(len(b)).encode() + b'\r\n\r\n' + b)
    sys.stdout.buffer.flush()
m = read()
send({'jsonrpc':'2.0','id':m['id'],'result':{'capabilities':{}}})
read()
m = read()
time.sleep(2)
"#,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&server, fs::Permissions::from_mode(0o700)).unwrap();
        let config = LspConfig::new(
            PathBuf::from(&server),
            Vec::new(),
            directory.clone(),
            None,
            None,
        )
        .unwrap();
        (directory, LspClient::start(&config, false).unwrap())
    }

    #[cfg(unix)]
    #[test]
    fn injected_cancellation_reaps_a_started_server_without_a_signal_fixture() {
        let (directory, mut client) = stalled_client();
        let pid = client.process.child.id();
        let start = Instant::now();
        client.process.stop.store(true, Ordering::Release);
        assert!(
            client
                .request("test/stall", json!({}))
                .unwrap_err()
                .contains("cancelled")
        );
        drop(client);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(
            unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stalled_lsp_request_is_time_bounded() {
        let (directory, mut client) = stalled_client();
        client.request_timeout = Duration::from_millis(100);
        let started = Instant::now();
        let error = client.request("test/stall", json!({})).unwrap_err();
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(client);
        fs::remove_dir_all(directory).unwrap();
    }
}
