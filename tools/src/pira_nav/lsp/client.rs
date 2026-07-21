use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

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
const MAX_SERVER_STDERR_BYTES: usize = 16 * 1024;

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
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_thread: Option<JoinHandle<()>>,
    next_id: u64,
    encoding: PositionEncoding,
    capabilities: ServerCapabilities,
    settings: Option<Value>,
    retain_documents: bool,
    open_documents: BTreeMap<String, Vec<usize>>,
}

impl LspClient {
    pub(super) fn start(config: &LspConfig, retain_documents: bool) -> Result<Self, String> {
        let mut child = Command::new(&config.executable)
            .args(&config.arguments)
            .current_dir(&config.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "cannot start LSP server {}: {error}",
                    config.executable.display()
                )
            })?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| "LSP server stdin was not available".to_string())?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| "LSP server stdout was not available".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "LSP server stderr was not available".to_string())?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_thread = Arc::clone(&captured);
        let stderr_thread = thread::spawn(move || {
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
        });
        let mut client = Self {
            child,
            input: BufWriter::new(input),
            output: BufReader::new(output),
            stderr: captured,
            stderr_thread: Some(stderr_thread),
            next_id: 1,
            encoding: PositionEncoding::Utf16,
            capabilities: ServerCapabilities::default(),
            settings: config.settings.clone(),
            retain_documents,
            open_documents: BTreeMap::new(),
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
                let next = parse_calls(&value, incoming, &uri, self.encoding)?;
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
        for _ in 0..MAX_MESSAGES_PER_REQUEST {
            let message = self.read()?;
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
        let id = message
            .get("id")
            .cloned()
            .ok_or_else(|| "LSP server request omitted id".to_string())?;
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let response = match method {
            "workspace/applyEdit" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"applied": false, "failureReason": "pira_nav is read-only"}
            }),
            "workspace/configuration" => {
                let items = message
                    .pointer("/params/items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let values = items
                    .iter()
                    .map(|item| match item.get("section") {
                        None | Some(Value::Null) => self.settings.clone().unwrap_or(Value::Null),
                        Some(Value::String(section)) => {
                            configuration_section(self.settings.as_ref(), section)
                                .unwrap_or(Value::Null)
                        }
                        Some(_) => Value::Null,
                    })
                    .collect::<Vec<_>>();
                json!({"jsonrpc": "2.0", "id": id, "result": values})
            }
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
        let payload = serde_json::to_vec(message)
            .map_err(|error| format!("cannot encode LSP message: {error}"))?;
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err(format!(
                "LSP message exceeds the {} MiB safety limit",
                MAX_MESSAGE_BYTES / (1024 * 1024)
            ));
        }
        write!(self.input, "Content-Length: {}\r\n\r\n", payload.len())
            .and_then(|_| self.input.write_all(&payload))
            .and_then(|_| self.input.flush())
            .map_err(|error| self.io_error("write", error))
    }

    fn read(&mut self) -> Result<Value, String> {
        let mut content_length = None;
        let mut header_bytes = 0usize;
        loop {
            let mut line = Vec::new();
            let count = self
                .output
                .read_until(b'\n', &mut line)
                .map_err(|error| self.io_error("read", error))?;
            if count == 0 {
                return Err(self.io_error("read", io::Error::from(io::ErrorKind::UnexpectedEof)));
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
        let length =
            content_length.ok_or_else(|| "LSP response omitted Content-Length".to_string())?;
        let mut payload = vec![0u8; length];
        self.output
            .read_exact(&mut payload)
            .map_err(|error| self.io_error("read", error))?;
        let value: Value = serde_json::from_slice(&payload)
            .map_err(|error| format!("invalid JSON from LSP server: {error}"))?;
        if !value.is_object() {
            return Err("LSP message must be a JSON object".into());
        }
        Ok(value)
    }

    fn io_error(&mut self, operation: &str, error: io::Error) -> String {
        let status = self
            .child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| format!("; server exited with {status}"))
            .unwrap_or_default();
        let stderr = self.stderr_excerpt();
        format!("cannot {operation} LSP message: {error}{status}{stderr}")
    }

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
        for (uri, _) in std::mem::take(&mut self.open_documents) {
            let _ = self.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": uri}}),
            );
        }
        let id = self.next_id;
        let _ = self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "shutdown",
            "params": null
        }));
        let _ = self.notify("exit", Value::Null);
        for _ in 0..100 {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}

fn configuration_section(settings: Option<&Value>, section: &str) -> Option<Value> {
    let mut value = settings?;
    if section.is_empty() {
        return Some(value.clone());
    }
    for component in section.split('.') {
        value = value.get(component)?;
    }
    Some(value.clone())
}

fn provider_enabled(value: Option<&Value>) -> bool {
    value.is_some_and(|provider| provider == true || provider.is_object())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::configuration_section;

    #[test]
    fn configuration_sections_support_whole_and_dotted_settings() {
        let settings = json!({"python": {"analysis": {"level": "strict"}}});
        assert_eq!(
            configuration_section(Some(&settings), ""),
            Some(settings.clone())
        );
        assert_eq!(
            configuration_section(Some(&settings), "python.analysis"),
            Some(json!({"level": "strict"}))
        );
        assert_eq!(
            configuration_section(Some(&settings), "python.missing"),
            None
        );
    }
}
