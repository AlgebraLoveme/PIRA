use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::language::Language;
use crate::model::Symbol;
use crate::util::{one_line, percent_decode, sanitize_metadata};

const MAX_SYMBOLS: usize = 100_000;
const MAX_LOCATIONS: usize = 100_000;
const MAX_SYMBOL_DEPTH: usize = 128;
const MAX_URI_BYTES: usize = 16 * 1024;
const MAX_HOVER_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
    Utf32,
}

impl PositionEncoding {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16 => "utf-16",
            Self::Utf32 => "utf-32",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LspPosition {
    pub line: usize,
    pub character: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

#[derive(Clone, Debug)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
    pub encoding: PositionEncoding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HoverFormat {
    PlainText,
    Markdown,
}

impl HoverFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlainText => "plaintext",
            Self::Markdown => "markdown",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LspHover {
    pub contents: String,
    pub format: HoverFormat,
    pub range: Option<LspRange>,
    pub encoding: PositionEncoding,
}

pub fn file_path_from_uri(uri: &str) -> Result<Option<PathBuf>, String> {
    let Some(rest) = uri.strip_prefix("file://") else {
        return Ok(None);
    };
    let path = if let Some(path) = rest.strip_prefix('/') {
        format!("/{path}")
    } else if let Some(path) = rest.strip_prefix("localhost/") {
        format!("/{path}")
    } else {
        return Ok(None);
    };
    let mut decoded =
        percent_decode(&path).map_err(|error| format!("invalid file URI: {error}"))?;
    if decoded.contains('\0') {
        return Err("file URI contains a null byte".into());
    }
    if cfg!(windows) && decoded.starts_with('/') && decoded.as_bytes().get(2) == Some(&b':') {
        decoded.remove(0);
    }
    Ok(Some(PathBuf::from(decoded)))
}

pub fn normalize_range(
    source: &str,
    range: LspRange,
    encoding: PositionEncoding,
) -> Result<LspRange, String> {
    let positions = SourcePositions::new(source, encoding);
    let (_, start_line, start_column) = positions.protocol_position(range.start)?;
    let (_, end_line, end_column) = positions.protocol_position(range.end)?;
    Ok(LspRange {
        start: LspPosition {
            line: start_line,
            character: start_column,
        },
        end: LspPosition {
            line: end_line,
            character: end_column,
        },
    })
}

pub(super) fn parse_locations(
    value: &Value,
    allow_links: bool,
    encoding: PositionEncoding,
) -> Result<Vec<LspLocation>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    if let Some(values) = value.as_array() {
        if values.len() > MAX_LOCATIONS {
            return Err("LSP location result exceeds the structural safety limit".into());
        }
        let mut locations = Vec::with_capacity(values.len());
        for value in values {
            locations.push(parse_location(value, allow_links, encoding)?);
        }
        return Ok(locations);
    }
    value
        .is_object()
        .then(|| parse_location(value, allow_links, encoding))
        .ok_or_else(|| "LSP location result is not an object, array, or null".to_string())?
        .map(|location| vec![location])
}

fn parse_location(
    value: &Value,
    allow_links: bool,
    encoding: PositionEncoding,
) -> Result<LspLocation, String> {
    let (uri, range) = if let Some(uri) = value.get("uri") {
        (uri, required(value, "range")?)
    } else if allow_links {
        (
            required(value, "targetUri")?,
            value
                .get("targetSelectionRange")
                .or_else(|| value.get("targetRange"))
                .ok_or_else(|| "LSP location link omitted its target range".to_string())?,
        )
    } else {
        return Err("LSP reference is not a location".into());
    };
    let uri = uri
        .as_str()
        .ok_or_else(|| "LSP location URI is not a string".to_string())?;
    if uri.len() > MAX_URI_BYTES {
        return Err("LSP location URI exceeds the safety limit".into());
    }
    Ok(LspLocation {
        uri: uri.to_string(),
        range: parse_protocol_range(range)?,
        encoding,
    })
}

fn parse_protocol_range(value: &Value) -> Result<LspRange, String> {
    let start = parse_protocol_position(required(value, "start")?)?;
    let end = parse_protocol_position(required(value, "end")?)?;
    if (end.line, end.character) < (start.line, start.character) {
        return Err("LSP range is reversed".into());
    }
    Ok(LspRange { start, end })
}

fn parse_protocol_position(value: &Value) -> Result<LspPosition, String> {
    let line = required(value, "line")?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "LSP position line is not a valid integer".to_string())?;
    let character = required(value, "character")?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "LSP position character is not a valid integer".to_string())?;
    Ok(LspPosition { line, character })
}

pub(super) fn parse_hover(
    value: &Value,
    encoding: PositionEncoding,
) -> Result<Option<LspHover>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let object = value
        .as_object()
        .ok_or_else(|| "LSP hover result is not an object or null".to_string())?;
    let (contents, format) = parse_hover_contents(
        object
            .get("contents")
            .ok_or_else(|| "LSP hover result omitted contents".to_string())?,
    )?;
    if contents.len() > MAX_HOVER_BYTES {
        return Err("LSP hover contents exceed the 1 MiB safety limit".into());
    }
    let range = object.get("range").map(parse_protocol_range).transpose()?;
    Ok(Some(LspHover {
        contents,
        format,
        range,
        encoding,
    }))
}

fn parse_hover_contents(value: &Value) -> Result<(String, HoverFormat), String> {
    if let Some(text) = value.as_str() {
        return Ok((text.to_string(), HoverFormat::Markdown));
    }
    if let Some(values) = value.as_array() {
        let mut parts = Vec::with_capacity(values.len());
        let mut format = HoverFormat::PlainText;
        for value in values {
            let (part, part_format) = parse_hover_contents(value)?;
            if part_format == HoverFormat::Markdown {
                format = HoverFormat::Markdown;
            }
            parts.push(part);
        }
        return Ok((parts.join("\n\n"), format));
    }
    let object = value
        .as_object()
        .ok_or_else(|| "LSP hover contents have an unsupported shape".to_string())?;
    let text = object
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| "LSP hover content object omitted string value".to_string())?;
    match object.get("kind").and_then(Value::as_str) {
        Some("plaintext") => Ok((text.to_string(), HoverFormat::PlainText)),
        Some("markdown") => Ok((text.to_string(), HoverFormat::Markdown)),
        Some(other) => Err(format!(
            "unsupported LSP hover markup kind: {}",
            bounded_text(other, 128)
        )),
        None => {
            let language = object
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let language = one_line(&bounded_text(language, 64)).replace('`', "");
            Ok((format!("```{language}\n{text}\n```"), HoverFormat::Markdown))
        }
    }
}

pub(super) fn parse_document_symbols(
    value: &Value,
    uri: &str,
    source: &str,
    language: Language,
    encoding: PositionEncoding,
) -> Result<Vec<Symbol>, String> {
    let Some(entries) = value.as_array() else {
        return value
            .is_null()
            .then(Vec::new)
            .ok_or_else(|| "LSP document symbols result is not an array or null".into());
    };
    let positions = SourcePositions::new(source, encoding);
    let mut symbols = Vec::new();
    for entry in entries {
        if entry.get("location").is_some() {
            push_flat_symbol(entry, uri, language, &positions, &mut symbols)?;
        } else {
            push_document_symbol(entry, None, 0, language, &positions, &mut symbols)?;
        }
    }
    symbols.sort_by_key(|symbol| (symbol.start_byte, symbol.end_byte, symbol.depth));
    symbols.dedup_by(|left, right| {
        left.start_byte == right.start_byte
            && left.end_byte == right.end_byte
            && left.kind == right.kind
            && left.qualified_name == right.qualified_name
    });
    Ok(symbols)
}

fn push_document_symbol(
    value: &Value,
    parent: Option<&str>,
    depth: usize,
    language: Language,
    positions: &SourcePositions<'_>,
    output: &mut Vec<Symbol>,
) -> Result<(), String> {
    if depth > MAX_SYMBOL_DEPTH || output.len() >= MAX_SYMBOLS {
        return Err("LSP document symbols exceed structural safety limits".into());
    }
    let name = symbol_name(value)?;
    let qualified = qualify_lsp(parent, &name, language);
    let (start_byte, end_byte, start_row, start_column, end_row, end_column) =
        positions.range(required(value, "range")?)?;
    output.push(Symbol {
        kind: symbol_kind(value.get("kind").and_then(Value::as_u64).unwrap_or(0)),
        qualified_name: qualified.clone(),
        signature: bounded_text(
            value.get("detail").and_then(Value::as_str).unwrap_or(&name),
            4 * 1024,
        ),
        start_byte,
        end_byte,
        start_row,
        start_column,
        end_row,
        end_column,
        depth,
    });
    if let Some(children) = value.get("children").and_then(Value::as_array) {
        for child in children {
            push_document_symbol(
                child,
                Some(&qualified),
                depth + 1,
                language,
                positions,
                output,
            )?;
        }
    }
    Ok(())
}

fn push_flat_symbol(
    value: &Value,
    uri: &str,
    language: Language,
    positions: &SourcePositions<'_>,
    output: &mut Vec<Symbol>,
) -> Result<(), String> {
    if output.len() >= MAX_SYMBOLS {
        return Err("LSP document symbols exceed structural safety limits".into());
    }
    let location = required(value, "location")?;
    if location.get("uri").and_then(Value::as_str) != Some(uri) {
        return Ok(());
    }
    let name = symbol_name(value)?;
    let container = value.get("containerName").and_then(Value::as_str);
    let qualified = qualify_lsp(container, &name, language);
    let (start_byte, end_byte, start_row, start_column, end_row, end_column) =
        positions.range(required(location, "range")?)?;
    output.push(Symbol {
        kind: symbol_kind(value.get("kind").and_then(Value::as_u64).unwrap_or(0)),
        qualified_name: qualified,
        signature: bounded_text(&name, 4 * 1024),
        start_byte,
        end_byte,
        start_row,
        start_column,
        end_row,
        end_column,
        depth: 0,
    });
    Ok(())
}

pub(super) struct SourcePositions<'a> {
    source: &'a str,
    line_starts: Vec<usize>,
    encoding: PositionEncoding,
}

impl<'a> SourcePositions<'a> {
    pub(super) fn new(source: &'a str, encoding: PositionEncoding) -> Self {
        let mut line_starts = vec![0];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Self {
            source,
            line_starts,
            encoding,
        }
    }

    fn range(&self, value: &Value) -> Result<(usize, usize, usize, usize, usize, usize), String> {
        let (start_byte, start_row, start_column) = self.position(required(value, "start")?)?;
        let (end_byte, end_row, end_column) = self.position(required(value, "end")?)?;
        if end_byte <= start_byte {
            return Err("LSP symbol has an empty or reversed range".into());
        }
        Ok((
            start_byte,
            end_byte,
            start_row,
            start_column,
            end_row,
            end_column,
        ))
    }

    fn position(&self, value: &Value) -> Result<(usize, usize, usize), String> {
        let line = required(value, "line")?
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "LSP position line is not a valid integer".to_string())?;
        let character = required(value, "character")?
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "LSP position character is not a valid integer".to_string())?;
        self.protocol_position(LspPosition { line, character })
    }

    fn protocol_position(&self, value: LspPosition) -> Result<(usize, usize, usize), String> {
        let row = value.line;
        let units = value.character;
        let start = *self
            .line_starts
            .get(row)
            .ok_or_else(|| format!("LSP position line {} is outside the source", row + 1))?;
        let mut end = self
            .line_starts
            .get(row + 1)
            .copied()
            .unwrap_or(self.source.len());
        if end > start && self.source.as_bytes()[end - 1] == b'\n' {
            end -= 1;
            if end > start && self.source.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
        }
        let line = &self.source[start..end];
        let byte_column = match self.encoding {
            PositionEncoding::Utf8 => {
                if units > line.len() || !line.is_char_boundary(units) {
                    return Err("LSP UTF-8 position splits or exceeds a source character".into());
                }
                units
            }
            PositionEncoding::Utf16 => encoded_column(line, units, char::len_utf16)?,
            PositionEncoding::Utf32 => encoded_column(line, units, |_| 1)?,
        };
        Ok((start + byte_column, row, byte_column))
    }

    pub(super) fn lsp_position(
        &self,
        row: usize,
        byte_column: usize,
    ) -> Result<LspPosition, String> {
        let start = *self
            .line_starts
            .get(row)
            .ok_or_else(|| format!("source line {} is outside the file", row + 1))?;
        let mut end = self
            .line_starts
            .get(row + 1)
            .copied()
            .unwrap_or(self.source.len());
        if end > start && self.source.as_bytes()[end - 1] == b'\n' {
            end -= 1;
            if end > start && self.source.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
        }
        let line = &self.source[start..end];
        if byte_column > line.len() || !line.is_char_boundary(byte_column) {
            return Err(format!(
                "source column {} splits or exceeds a character on line {}",
                byte_column + 1,
                row + 1
            ));
        }
        let prefix = &line[..byte_column];
        let character = match self.encoding {
            PositionEncoding::Utf8 => byte_column,
            PositionEncoding::Utf16 => prefix.encode_utf16().count(),
            PositionEncoding::Utf32 => prefix.chars().count(),
        };
        Ok(LspPosition {
            line: row,
            character,
        })
    }
}

fn encoded_column(
    line: &str,
    requested: usize,
    width: impl Fn(char) -> usize,
) -> Result<usize, String> {
    let mut units = 0usize;
    for (byte, character) in line.char_indices() {
        if units == requested {
            return Ok(byte);
        }
        units += width(character);
        if units > requested {
            return Err("LSP position splits a multi-unit source character".into());
        }
    }
    if units == requested {
        Ok(line.len())
    } else {
        Err("LSP position exceeds its source line".into())
    }
}

fn required<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value
        .get(key)
        .ok_or_else(|| format!("LSP document symbol omitted {key}"))
}

fn symbol_name(value: &Value) -> Result<String, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(|name| bounded_text(name, 1024))
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "LSP document symbol has no usable name".to_string())?;
    Ok(name)
}

pub(super) fn bounded_text(value: &str, max_bytes: usize) -> String {
    let mut text = one_line(&sanitize_metadata(value));
    if text.len() > max_bytes {
        let mut end = max_bytes;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

fn qualify_lsp(parent: Option<&str>, name: &str, language: Language) -> String {
    let Some(parent) = parent.filter(|parent| !parent.is_empty()) else {
        return name.to_string();
    };
    if name == parent
        || name
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with(['.', ':', '\\']))
    {
        return name.to_string();
    }
    format!("{parent}{}{name}", qualification_separator(language))
}

fn qualification_separator(language: Language) -> &'static str {
    match language {
        Language::Rust | Language::C | Language::Cpp | Language::Cuda | Language::PowerShell => {
            "::"
        }
        Language::Php => "\\",
        _ => ".",
    }
}

fn symbol_kind(kind: u64) -> &'static str {
    match kind {
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "binding",
        14 => "constant",
        22 => "variant",
        23 => "struct",
        25 => "operator",
        26 => "type",
        _ => "symbol",
    }
}

pub(super) fn language_id(language: Language) -> &'static str {
    match language {
        Language::Bash => "shellscript",
        Language::Cuda => "cuda-cpp",
        Language::Hcl => "terraform",
        _ => language.name(),
    }
}

pub(super) fn file_uri(path: &Path) -> Result<String, String> {
    let mut path = path
        .to_str()
        .ok_or_else(|| format!("LSP path is not valid UTF-8: {}", path.display()))?
        .replace('\\', "/");
    if cfg!(windows) && !path.starts_with('/') {
        path.insert(0, '/');
    }
    let mut uri = String::with_capacity(path.len() + 8);
    uri.push_str("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'.' | b'_' | b'~') {
            uri.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    Ok(uri)
}

#[cfg(test)]
mod tests {
    use super::{PositionEncoding, SourcePositions, file_uri};

    #[test]
    fn utf16_positions_map_to_utf8_byte_columns() {
        let source = "aé😀z\n";
        let positions = SourcePositions::new(source, PositionEncoding::Utf16);
        let value = serde_json::json!({"line": 0, "character": 4});
        assert_eq!(positions.position(&value).unwrap(), (7, 0, 7));
    }

    #[test]
    fn positions_exclude_crlf_line_endings() {
        let positions = SourcePositions::new("a\r\nb", PositionEncoding::Utf16);
        let end = serde_json::json!({"line": 0, "character": 1});
        let past_end = serde_json::json!({"line": 0, "character": 2});
        assert_eq!(positions.position(&end).unwrap(), (1, 0, 1));
        assert!(positions.position(&past_end).is_err());
    }

    #[test]
    fn file_uri_escapes_spaces_and_unicode() {
        let uri = file_uri(std::path::Path::new("/tmp/naïve file.rs")).unwrap();
        assert_eq!(uri, "file:///tmp/na%C3%AFve%20file.rs");
    }
}
