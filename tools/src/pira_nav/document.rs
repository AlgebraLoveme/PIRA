use std::borrow::Cow;
use std::collections::BTreeMap;

use tree_sitter::{Node, Tree};

use crate::language::Language;
use crate::model::{MAX_SYMBOL_TEXT_BYTES, Symbol, SymbolPath};
use crate::util::{one_line, source_slice};

pub const MAX_DOCUMENT_SYMBOLS: usize = 20_000;

pub struct DocumentSymbols {
    pub symbols: Vec<Symbol>,
    pub truncated: bool,
}

pub fn parse_input(language: Language, source: &str) -> Cow<'_, str> {
    if language == Language::Jsonc {
        normalize_jsonc_trailing_commas(source)
    } else {
        Cow::Borrowed(source)
    }
}

pub fn collect(tree: &Tree, language: Language, source: &str) -> DocumentSymbols {
    let mut collector = Collector::new(source);
    let root = SymbolPath::default();
    match language {
        Language::Json | Language::Jsonc => {
            for child in named_children(tree.root_node()) {
                walk_json_value(child, &root, 0, &mut collector);
            }
        }
        Language::Yaml => walk_yaml_stream(tree.root_node(), &mut collector),
        Language::Toml => walk_toml_document(tree.root_node(), &mut collector),
        _ => unreachable!("document collector requires a structured-document language"),
    }
    DocumentSymbols {
        symbols: collector.symbols,
        truncated: collector.truncated || collector.incomplete,
    }
}

#[derive(Clone)]
struct MarkdownHeading {
    level: usize,
    title: String,
    start_byte: usize,
    heading_end_byte: usize,
    start_row: usize,
}

pub fn collect_markdown(source: &str) -> DocumentSymbols {
    let headings = markdown_headings(source);
    let mut truncated = headings.len() > MAX_DOCUMENT_SYMBOLS;
    // Determine boundaries before applying the output inventory limit.
    let mut ends = vec![source.len(); headings.len()];
    let mut open = Vec::<usize>::new();
    for (index, heading) in headings.iter().enumerate() {
        while open
            .last()
            .is_some_and(|previous| headings[*previous].level >= heading.level)
        {
            ends[open.pop().unwrap()] = heading.start_byte;
        }
        open.push(index);
    }
    let line_starts: Vec<_> = std::iter::once(0)
        .chain(
            source
                .bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
        )
        .collect();
    let mut hierarchy = Vec::<(usize, String)>::new();
    let mut symbols = Vec::with_capacity(headings.len().min(MAX_DOCUMENT_SYMBOLS));
    let mut text_bytes = 0usize;
    for (index, heading) in headings.iter().take(MAX_DOCUMENT_SYMBOLS).enumerate() {
        while hierarchy
            .last()
            .is_some_and(|(level, _)| *level >= heading.level)
        {
            hierarchy.pop();
        }
        hierarchy.push((heading.level, heading.title.clone()));
        if hierarchy.iter().map(|(_, name)| name.len()).sum::<usize>()
            > MAX_SYMBOL_TEXT_BYTES.saturating_sub(text_bytes)
        {
            truncated = true;
            break;
        }
        let path = SymbolPath::from_names(hierarchy.iter().map(|(_, title)| title.clone()));
        let qualified_name = path.canonical();
        let legacy_qualified_name = hierarchy
            .iter()
            .map(|(_, title)| title.as_str())
            .collect::<Vec<_>>()
            .join(" > ");
        let end_byte = trim_markdown_section_end(source, heading.heading_end_byte, ends[index]);
        let end_row = line_starts.partition_point(|start| *start <= end_byte) - 1;
        let end_column = end_byte - line_starts[end_row];
        let symbol = Symbol {
            kind: match heading.level {
                1 => "heading1",
                2 => "heading2",
                3 => "heading3",
                4 => "heading4",
                5 => "heading5",
                _ => "heading6",
            },
            path,
            qualified_name,
            legacy_qualified_name,
            signature: heading.title.clone(),
            name_position: None,
            start_byte: heading.start_byte,
            end_byte,
            start_row: heading.start_row,
            start_column: 0,
            end_row,
            end_column,
            depth: heading.level - 1,
        };
        if symbol.text_bytes() > MAX_SYMBOL_TEXT_BYTES.saturating_sub(text_bytes) {
            truncated = true;
            break;
        }
        text_bytes += symbol.text_bytes();
        symbols.push(symbol);
    }
    DocumentSymbols { symbols, truncated }
}

fn markdown_headings(source: &str) -> Vec<MarkdownHeading> {
    let mut headings = Vec::new();
    let mut fence = None::<(u8, usize)>;
    let mut previous = None::<(usize, usize, String)>;
    let mut offset = 0usize;
    for (row, raw_line) in source.split_inclusive('\n').enumerate() {
        let line = raw_line.trim_end_matches(['\n', '\r']);
        let line_end = offset + raw_line.len();
        let content_end = offset + line.len();
        if let Some((marker, length, closing)) = markdown_fence(line) {
            match fence {
                Some((open_marker, open_length))
                    if closing && marker == open_marker && length >= open_length =>
                {
                    fence = None;
                }
                None => fence = Some((marker, length)),
                _ => {}
            }
            previous = None;
            offset = line_end;
            continue;
        }
        if fence.is_some() {
            previous = None;
            offset = line_end;
            continue;
        }
        if let Some((level, title)) = markdown_atx_heading(line) {
            headings.push(MarkdownHeading {
                level,
                title,
                start_byte: offset,
                heading_end_byte: content_end,
                start_row: row,
            });
            previous = None;
        } else if let Some(level) = markdown_setext_level(line) {
            if let Some((start_byte, start_row, title)) = previous.take() {
                headings.push(MarkdownHeading {
                    level,
                    title,
                    start_byte,
                    heading_end_byte: content_end,
                    start_row,
                });
            }
        } else {
            previous = markdown_setext_candidate(line).map(|title| (offset, row, title));
        }
        offset = line_end;
    }
    headings
}

fn markdown_atx_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len().saturating_sub(trimmed.len()) > 3 {
        return None;
    }
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level)
        || trimmed
            .as_bytes()
            .get(level)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }
    let mut title = trimmed[level..].trim();
    let without_hashes = title.trim_end_matches('#');
    if without_hashes.len() < title.len()
        && without_hashes
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_whitespace)
    {
        title = without_hashes.trim_end();
    }
    Some((level, markdown_title(title, level)))
}

fn markdown_setext_level(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    if line
        .len()
        .saturating_sub(line.trim_start_matches(' ').len())
        > 3
        || trimmed.is_empty()
    {
        return None;
    }
    if trimmed.bytes().all(|byte| byte == b'=') {
        Some(1)
    } else if trimmed.bytes().all(|byte| byte == b'-') {
        Some(2)
    } else {
        None
    }
}

fn markdown_setext_candidate(line: &str) -> Option<String> {
    let trimmed = line.trim();
    (!trimmed.is_empty()
        && line
            .len()
            .saturating_sub(line.trim_start_matches(' ').len())
            <= 3)
        .then(|| trimmed.to_string())
}

fn markdown_title(title: &str, level: usize) -> String {
    if title.is_empty() {
        format!("(untitled h{level})")
    } else {
        title.to_string()
    }
}

fn markdown_fence(line: &str) -> Option<(u8, usize, bool)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len().saturating_sub(trimmed.len()) > 3 {
        return None;
    }
    let marker = *trimmed.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = trimmed.bytes().take_while(|byte| *byte == marker).count();
    if length < 3 {
        return None;
    }
    let closing = trimmed[length..].trim().is_empty();
    Some((marker, length, closing))
}

fn trim_markdown_section_end(source: &str, minimum: usize, end: usize) -> usize {
    let mut result = end;
    while result > minimum && matches!(source.as_bytes()[result - 1], b'\n' | b'\r') {
        result -= 1;
    }
    result.max(minimum)
}

struct Collector<'a> {
    source: &'a str,
    symbols: Vec<Symbol>,
    text_bytes: usize,
    truncated: bool,
    incomplete: bool,
}

impl<'a> Collector<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            symbols: Vec::new(),
            text_bytes: 0,
            truncated: false,
            incomplete: false,
        }
    }

    fn push(&mut self, node: Node<'_>, path: SymbolPath, kind: &'static str, depth: usize) {
        if self.truncated
            || self.symbols.len() >= MAX_DOCUMENT_SYMBOLS
            || path.text_bytes() > MAX_SYMBOL_TEXT_BYTES.saturating_sub(self.text_bytes)
        {
            self.truncated = true;
            return;
        }
        let qualified_name = path.canonical();
        let legacy_qualified_name = path.legacy_document();
        let symbol = Symbol {
            kind,
            path,
            qualified_name,
            legacy_qualified_name,
            signature: document_signature(node, self.source),
            name_position: None,
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            start_row: node.start_position().row,
            start_column: node.start_position().column,
            end_row: node.end_position().row,
            end_column: node.end_position().column,
            depth,
        };
        if symbol.text_bytes() > MAX_SYMBOL_TEXT_BYTES.saturating_sub(self.text_bytes) {
            self.truncated = true;
            return;
        }
        self.text_bytes += symbol.text_bytes();
        self.symbols.push(symbol);
    }
}

fn walk_json_value(node: Node<'_>, parent: &SymbolPath, depth: usize, output: &mut Collector<'_>) {
    if output.truncated {
        return;
    }
    match node.kind() {
        "object" => {
            for pair in named_children(node).filter(|child| child.kind() == "pair") {
                if output.truncated {
                    break;
                }
                let (Some(key_node), Some(value)) = (
                    pair.child_by_field_name("key"),
                    pair.child_by_field_name("value"),
                ) else {
                    continue;
                };
                let Some(key) = json_string(key_node, output.source) else {
                    continue;
                };
                let path = parent.child_name(key);
                output.push(pair, path.clone(), "key", depth);
                walk_json_value(value, &path, depth + 1, output);
            }
        }
        "array" => {
            for (index, value) in named_children(node)
                .filter(|child| child.kind() != "comment")
                .enumerate()
            {
                if output.truncated {
                    break;
                }
                let path = parent.child_index(index);
                output.push(value, path.clone(), "item", depth);
                walk_json_value(value, &path, depth + 1, output);
            }
        }
        _ => {}
    }
}

fn walk_yaml_stream(root: Node<'_>, output: &mut Collector<'_>) {
    let documents = named_children(root)
        .filter(|child| child.kind() == "document")
        .collect::<Vec<_>>();
    let multiple = documents.len() > 1;
    for (index, document) in documents.into_iter().enumerate() {
        if output.truncated {
            break;
        }
        let path = multiple.then(|| SymbolPath::from_names(["document".into()]).child_index(index));
        if let Some(path) = &path {
            output.push(document, path.clone(), "document", 0);
        }
        if let Some(value) = yaml_payload(document) {
            walk_yaml_value(
                value,
                path.as_ref().unwrap_or(&SymbolPath::default()),
                usize::from(multiple),
                output,
            );
        }
    }
    collect_yaml_references(root, output);
}

fn walk_yaml_value(node: Node<'_>, parent: &SymbolPath, depth: usize, output: &mut Collector<'_>) {
    if output.truncated {
        return;
    }
    let node = yaml_payload(node).unwrap_or(node);
    match node.kind() {
        "block_mapping" | "flow_mapping" => {
            for pair in named_children(node)
                .filter(|child| matches!(child.kind(), "block_mapping_pair" | "flow_pair"))
            {
                if output.truncated {
                    break;
                }
                walk_yaml_pair(pair, parent, depth, output);
            }
        }
        "block_mapping_pair" | "flow_pair" => walk_yaml_pair(node, parent, depth, output),
        "block_sequence" | "flow_sequence" => {
            let values = named_children(node)
                .filter(|child| !matches!(child.kind(), "comment" | "anchor" | "tag"));
            for (index, item) in values.enumerate() {
                if output.truncated {
                    break;
                }
                let value = if item.kind() == "block_sequence_item" {
                    yaml_payload(item).unwrap_or(item)
                } else {
                    item
                };
                let path = parent.child_index(index);
                output.push(item, path.clone(), "item", depth);
                walk_yaml_value(value, &path, depth + 1, output);
            }
        }
        _ => {}
    }
}

fn walk_yaml_pair(pair: Node<'_>, parent: &SymbolPath, depth: usize, output: &mut Collector<'_>) {
    let Some(key_node) = pair.child_by_field_name("key") else {
        return;
    };
    let Some(key) = yaml_scalar(key_node, output.source) else {
        output.incomplete = true;
        return;
    };
    let path = parent.child_name(key);
    output.push(pair, path.clone(), "key", depth);
    if let Some(value) = pair.child_by_field_name("value") {
        walk_yaml_value(value, &path, depth + 1, output);
    }
}

fn yaml_payload(node: Node<'_>) -> Option<Node<'_>> {
    if matches!(
        node.kind(),
        "block_mapping"
            | "flow_mapping"
            | "block_sequence"
            | "flow_sequence"
            | "block_mapping_pair"
            | "flow_pair"
    ) {
        return Some(node);
    }
    named_children(node)
        .filter(|child| !matches!(child.kind(), "anchor" | "tag" | "comment"))
        .find_map(yaml_payload)
}

fn yaml_scalar(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "plain_scalar" | "single_quote_scalar" | "double_quote_scalar" => decode_quoted_scalar(
            source_slice(source, node.start_byte(), node.end_byte()).as_ref(),
            Language::Yaml,
        ),
        "flow_node" | "block_node" => named_children(node)
            .find(|child| !matches!(child.kind(), "anchor" | "tag" | "comment"))
            .and_then(|child| yaml_scalar(child, source)),
        // PIRA: complex/block-scalar keys have no scalar-path representation here;
        // callers disclose an incomplete inventory and retain parser-free access.
        _ => None,
    }
}

fn collect_yaml_references(node: Node<'_>, output: &mut Collector<'_>) {
    if output.truncated {
        return;
    }
    if matches!(node.kind(), "anchor" | "alias")
        && let Some(name) = named_children(node).next()
    {
        let raw = source_slice(output.source, name.start_byte(), name.end_byte());
        let prefix = if node.kind() == "anchor" { '&' } else { '*' };
        output.push(
            node,
            SymbolPath::from_names([format!("{prefix}{raw}")]),
            node.kind(),
            0,
        );
        return;
    }
    for child in named_children(node) {
        collect_yaml_references(child, output);
    }
}

fn walk_toml_document(root: Node<'_>, output: &mut Collector<'_>) {
    let mut table_arrays = BTreeMap::<String, usize>::new();
    let root_path = SymbolPath::default();
    for child in named_children(root) {
        if output.truncated {
            break;
        }
        match child.kind() {
            "pair" => walk_toml_pair(child, &root_path, 0, output),
            "table" | "table_array_element" => {
                let Some(key_node) = named_children(child).find(|node| is_toml_key(*node)) else {
                    continue;
                };
                let Some(segments) = toml_key_segments(key_node, output.source) else {
                    output.incomplete = true;
                    continue;
                };
                let mut base = root_path.clone();
                let mut segments = segments.into_iter().peekable();
                while let Some(segment) = segments.next() {
                    base = base.child_name(segment);
                    // Subtables belong to the latest occurrence of each array ancestor.
                    // Including those indices also scopes nested array counters correctly.
                    if segments.peek().is_some()
                        && let Some(count) = table_arrays.get(&base.canonical())
                    {
                        base = base.child_index(count - 1);
                    }
                }
                let (path, kind) = if child.kind() == "table_array_element" {
                    let index = table_arrays.entry(base.canonical()).or_default();
                    let path = base.child_index(*index);
                    *index += 1;
                    (path, "table-item")
                } else {
                    (base, "table")
                };
                output.push(child, path.clone(), kind, 0);
                for pair in named_children(child).filter(|node| node.kind() == "pair") {
                    if output.truncated {
                        break;
                    }
                    walk_toml_pair(pair, &path, 1, output);
                }
            }
            _ => {}
        }
    }
}

fn walk_toml_pair(pair: Node<'_>, parent: &SymbolPath, depth: usize, output: &mut Collector<'_>) {
    let mut children = named_children(pair);
    let Some(key_node) = children.find(|node| is_toml_key(*node)) else {
        return;
    };
    let Some(segments) = toml_key_segments(key_node, output.source) else {
        output.incomplete = true;
        return;
    };
    let path = parent.extend_names(segments);
    output.push(pair, path.clone(), "key", depth);
    if let Some(value) = named_children(pair).find(|node| !is_toml_key(*node)) {
        walk_toml_value(value, &path, depth + 1, output);
    }
}

fn walk_toml_value(node: Node<'_>, parent: &SymbolPath, depth: usize, output: &mut Collector<'_>) {
    if output.truncated {
        return;
    }
    match node.kind() {
        "inline_table" => {
            for pair in named_children(node).filter(|child| child.kind() == "pair") {
                if output.truncated {
                    break;
                }
                walk_toml_pair(pair, parent, depth, output);
            }
        }
        "array" => {
            for (index, value) in named_children(node)
                .filter(|child| child.kind() != "comment")
                .enumerate()
            {
                if output.truncated {
                    break;
                }
                let path = parent.child_index(index);
                output.push(value, path.clone(), "item", depth);
                walk_toml_value(value, &path, depth + 1, output);
            }
        }
        _ => {}
    }
}

fn is_toml_key(node: Node<'_>) -> bool {
    matches!(node.kind(), "bare_key" | "quoted_key" | "dotted_key")
}

fn toml_key_segments(node: Node<'_>, source: &str) -> Option<Vec<String>> {
    match node.kind() {
        "bare_key" => Some(vec![
            source_slice(source, node.start_byte(), node.end_byte()).into_owned(),
        ]),
        "quoted_key" => decode_quoted_scalar(
            source_slice(source, node.start_byte(), node.end_byte()).as_ref(),
            Language::Toml,
        )
        .map(|key| vec![key]),
        "dotted_key" => {
            let segments = named_children(node)
                .filter(|child| is_toml_key(*child))
                .map(|child| toml_key_segments(child, source))
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            (!segments.is_empty()).then_some(segments)
        }
        _ => None,
    }
}

fn json_string(node: Node<'_>, source: &str) -> Option<String> {
    serde_json::from_str(source_slice(source, node.start_byte(), node.end_byte()).as_ref()).ok()
}

pub(crate) fn decode_quoted_scalar(raw: &str, language: Language) -> Option<String> {
    let value = raw.trim();
    if language == Language::R {
        return decode_r_string(value);
    }
    let yaml = language == Language::Yaml;
    let javascript = matches!(language, Language::JavaScript | Language::TypeScript);
    if value.starts_with('\'') && value.ends_with('\'') {
        let body = value.get(1..value.len().checked_sub(1)?)?;
        if !yaml && !javascript {
            return Some(body.to_owned());
        }
        if yaml {
            return Some(fold_yaml_flow(&body.replace("''", "'")));
        }
    } else if !(value.starts_with('"') && value.ends_with('"')) {
        return yaml.then(|| fold_yaml_flow(value));
    }
    let body = value.get(1..value.len().checked_sub(1)?)?;
    // Normalize physical YAML line endings before flow folding, not decoded escapes.
    let normalized;
    let body = if yaml {
        normalized = body.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        body
    };
    let mut chars = body.chars().peekable();
    let mut output = String::new();
    while let Some(character) = chars.next() {
        match character {
            '\n' if yaml => fold_yaml_break(&mut chars, &mut output),
            ' ' | '\t' if yaml => {
                let mut space = String::from(character);
                while matches!(chars.peek(), Some(' ' | '\t')) {
                    space.push(chars.next()?);
                }
                if chars.peek() != Some(&'\n') {
                    output.push_str(&space);
                }
            }
            '\\' => {
                let escaped = chars.next()?;
                let decoded = match escaped {
                    '\r' if javascript => {
                        if chars.peek() == Some(&'\n') {
                            chars.next();
                        }
                        continue;
                    }
                    '\n' if yaml || javascript => {
                        if yaml {
                            while matches!(chars.peek(), Some(' ' | '\t' | '\n')) {
                                if chars.next()? == '\n' {
                                    output.push('\n');
                                }
                            }
                        }
                        continue;
                    }
                    '0' if yaml || javascript => '\0',
                    'a' if yaml => '\u{7}',
                    'b' => '\u{8}',
                    't' => '\t',
                    'n' => '\n',
                    'v' if yaml || javascript => '\u{b}',
                    'f' => '\u{c}',
                    'r' => '\r',
                    'e' if yaml => '\u{1b}',
                    ' ' | '\t' if yaml => escaped,
                    '"' | '\\' => escaped,
                    '/' if yaml || javascript => '/',
                    '\'' if javascript => '\'',
                    'N' if yaml => '\u{85}',
                    '_' if yaml => '\u{a0}',
                    'L' if yaml => '\u{2028}',
                    'P' if yaml => '\u{2029}',
                    'x' if yaml || javascript => decode_hex(&mut chars, 2)?,
                    'u' if javascript && chars.peek() == Some(&'{') => {
                        chars.next();
                        let mut digits = String::new();
                        while chars.peek() != Some(&'}') {
                            digits.push(chars.next()?);
                        }
                        chars.next();
                        char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?
                    }
                    'u' => {
                        let high = hex_value(&mut chars, 4)?;
                        let scalar = if javascript && (0xd800..=0xdbff).contains(&high) {
                            if chars.next()? != '\\' || chars.next()? != 'u' {
                                return None;
                            }
                            let low = hex_value(&mut chars, 4)?;
                            if !(0xdc00..=0xdfff).contains(&low) {
                                return None;
                            }
                            0x10000 + ((high - 0xd800) << 10) + low - 0xdc00
                        } else {
                            high
                        };
                        char::from_u32(scalar)?
                    }
                    'U' if !javascript => decode_hex(&mut chars, 8)?,
                    other if javascript && !other.is_ascii_digit() => other,
                    _ => return None,
                };
                output.push(decoded);
            }
            other => output.push(other),
        }
    }
    Some(output)
}

// R byte escapes are bytes, not Unicode scalar escapes; decode the final UTF-8 name.
fn decode_r_string(value: &str) -> Option<String> {
    let quote = value.chars().next()?;
    if !matches!(quote, '\'' | '"' | '`') || !value.ends_with(quote) {
        return None;
    }
    let body = value.get(1..value.len().checked_sub(1)?)?;
    let mut chars = body.chars().peekable();
    let mut output = Vec::new();
    while let Some(character) = chars.next() {
        let character = if character == '\\' {
            match chars.next()? {
                'a' => '\u{7}',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'v' => '\u{b}',
                '\\' => '\\',
                '\'' => '\'',
                '"' => '"',
                '`' => '`',
                first @ '0'..='7' => {
                    let mut byte = first.to_digit(8)?;
                    for _ in 0..2 {
                        let Some(digit) = chars.peek().and_then(|c| c.to_digit(8)) else {
                            break;
                        };
                        chars.next();
                        byte = byte * 8 + digit;
                    }
                    output.push(u8::try_from(byte).ok()?);
                    continue;
                }
                'x' => {
                    output.push(u8::try_from(variable_hex(&mut chars, 2, false)?).ok()?);
                    continue;
                }
                escape @ ('u' | 'U') if quote != '`' => {
                    let braced = chars.peek() == Some(&'{');
                    if braced {
                        chars.next();
                    }
                    char::from_u32(variable_hex(
                        &mut chars,
                        if escape == 'u' { 4 } else { 8 },
                        braced,
                    )?)?
                }
                _ => return None,
            }
        } else {
            character
        };
        output.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
    }
    (!output.contains(&0))
        .then(|| String::from_utf8(output).ok())
        .flatten()
}

fn variable_hex(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    max: usize,
    braced: bool,
) -> Option<u32> {
    let mut value = 0u32;
    let mut count = 0;
    while count < max {
        let Some(digit) = chars.peek().and_then(|c| c.to_digit(16)) else {
            break;
        };
        chars.next();
        value = value.checked_mul(16)?.checked_add(digit)?;
        count += 1;
    }
    if count == 0 || (braced && chars.next()? != '}') {
        return None;
    }
    Some(value)
}

fn hex_value(chars: &mut impl Iterator<Item = char>, count: usize) -> Option<u32> {
    let mut value = 0u32;
    for _ in 0..count {
        value = value * 16 + chars.next()?.to_digit(16)?;
    }
    Some(value)
}

fn decode_hex(chars: &mut impl Iterator<Item = char>, count: usize) -> Option<char> {
    char::from_u32(hex_value(chars, count)?)
}

fn fold_yaml_break(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, output: &mut String) {
    let mut breaks = 1;
    while matches!(chars.peek(), Some(' ' | '\t' | '\n')) {
        if chars.next() == Some('\n') {
            breaks += 1;
        }
    }
    if breaks == 1 {
        output.push(' ');
    } else {
        output.extend(std::iter::repeat_n('\n', breaks - 1));
    }
}

fn fold_yaml_flow(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut chars = normalized.chars().peekable();
    let mut output = String::new();
    while let Some(character) = chars.next() {
        match character {
            '\n' => fold_yaml_break(&mut chars, &mut output),
            ' ' | '\t' => {
                let mut space = String::from(character);
                while matches!(chars.peek(), Some(' ' | '\t')) {
                    space.push(chars.next().unwrap());
                }
                if chars.peek() != Some(&'\n') {
                    output.push_str(&space);
                }
            }
            other => output.push(other),
        }
    }
    output
}

fn document_signature(node: Node<'_>, source: &str) -> String {
    const MAX_SIGNATURE_BYTES: usize = 256;
    let end = node.end_byte().min(node.start_byte() + MAX_SIGNATURE_BYTES);
    let prefix = source_slice(source, node.start_byte(), end);
    one_line(prefix.lines().next().unwrap_or_default())
}

fn named_children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let count = u32::try_from(node.named_child_count()).unwrap_or(u32::MAX);
    (0..count).filter_map(move |index| node.named_child(index))
}

fn normalize_jsonc_trailing_commas(source: &str) -> Cow<'_, str> {
    let bytes = source.as_bytes();
    let mut normalized = None::<Vec<u8>>;
    let mut index = 0;
    let mut string = false;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut previous = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
        } else if block_comment {
            if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                block_comment = false;
                index += 1;
            }
        } else if string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
                previous = Some(b'"');
            }
        } else if byte == b'"' {
            string = true;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            line_comment = true;
            index += 1;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            block_comment = true;
            index += 1;
        } else if byte == b','
            && previous.is_some_and(|token| !matches!(token, b'[' | b'{' | b':' | b','))
            && next_jsonc_token(bytes, index + 1).is_some_and(|next| matches!(next, b'}' | b']'))
        {
            normalized.get_or_insert_with(|| bytes.to_vec())[index] = b' ';
            previous = Some(byte);
        } else if !byte.is_ascii_whitespace() {
            previous = Some(byte);
        }
        index += 1;
    }
    normalized.map_or(Cow::Borrowed(source), |bytes| {
        Cow::Owned(String::from_utf8(bytes).expect("ASCII replacement preserves UTF-8"))
    })
}

fn next_jsonc_token(bytes: &[u8], mut index: usize) -> Option<u8> {
    loop {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index..index + 2) == Some(b"//") {
            index += 2;
            while bytes.get(index).is_some_and(|byte| *byte != b'\n') {
                index += 1;
            }
        } else if bytes.get(index..index + 2) == Some(b"/*") {
            index += 2;
            while bytes.get(index..index + 2) != Some(b"*/") {
                index += 1;
                if index >= bytes.len() {
                    return None;
                }
            }
            index += 2;
        } else {
            return bytes.get(index).copied();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn jsonc_normalization_preserves_offsets_and_ignores_string_commas() {
        let source = "{\n  \"text\": \",}\",\n  \"items\": [1, 2, // retained comment\n  ],\n}\n";
        let normalized = parse_input(Language::Jsonc, source);
        assert_eq!(source.len(), normalized.len());
        assert!(normalized.contains("\",}\","));
        assert!(!normalized.contains("2,"));
        assert_eq!(
            source.matches(',').count() - 2,
            normalized.matches(',').count()
        );
    }

    #[test]
    fn special_document_keys_have_unambiguous_paths() {
        let root = SymbolPath::from_names(["root".into()]);
        assert_eq!(root.child_name("plain-key").canonical(), "root::plain-key");
        assert_eq!(root.child_name("a.b").canonical(), "root::[\"a.b\"]");
        assert_eq!(
            root.child_name("items").child_index(2).canonical(),
            "root::items[2]"
        );
    }

    #[test]
    fn document_symbol_limit_marks_only_actual_omissions() {
        fn parse_items(count: usize) -> DocumentSymbols {
            let source = format!(
                "{{\"items\":[{}]}}",
                std::iter::repeat_n("0", count)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let mut parser = Language::Json.parser(Path::new("limit.json")).unwrap();
            let tree = parser.parse(&source, None).unwrap();
            collect(&tree, Language::Json, &source)
        }

        let exact = parse_items(MAX_DOCUMENT_SYMBOLS - 1);
        assert_eq!(exact.symbols.len(), MAX_DOCUMENT_SYMBOLS);
        assert!(!exact.truncated);

        let over = parse_items(MAX_DOCUMENT_SYMBOLS);
        assert_eq!(over.symbols.len(), MAX_DOCUMENT_SYMBOLS);
        assert!(over.truncated);
    }

    #[test]
    fn markdown_headings_are_hierarchical_section_ranges() {
        let source = "# Guide\nintro\n## Install ##\nsteps\n### Verify\ncheck\nConfiguration\n-------------\nsettings\n## Inspect\nnext\n";
        let parsed = collect_markdown(source);
        let names = parsed
            .symbols
            .iter()
            .map(|symbol| symbol.qualified_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "Guide",
                "Guide::Install",
                "Guide::Install::Verify",
                "Guide::Configuration",
                "Guide::Inspect",
            ]
        );
        let install = &parsed.symbols[1];
        let section = &source[install.start_byte..install.end_byte];
        assert!(section.contains("### Verify"));
        assert!(!section.contains("Configuration"));
        assert!(!parsed.truncated);
    }

    #[test]
    fn markdown_boundaries_survive_inventory_truncation() {
        let mut source = String::from("# A\n");
        source.push_str(&"## nested\n".repeat(MAX_DOCUMENT_SYMBOLS));
        let boundary = source.len();
        source.push_str("# B\nUNRELATED\n");
        let parsed = collect_markdown(&source);
        assert!(parsed.truncated);
        assert_eq!(parsed.symbols.len(), MAX_DOCUMENT_SYMBOLS);
        let section = &parsed.symbols[0];
        assert_eq!(section.end_byte, boundary - 1);
        assert_eq!(section.end_row, MAX_DOCUMENT_SYMBOLS);
        assert!(!source[..section.end_byte].contains("UNRELATED"));
    }

    #[test]
    fn markdown_fences_hide_heading_like_content() {
        let source =
            "# Visible\n```\n# Hidden\nFake\n----\n```\n~~~text\n## Also hidden\n~~~\n## Shown\n";
        let parsed = collect_markdown(source);
        let names = parsed
            .symbols
            .iter()
            .map(|symbol| symbol.qualified_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Visible", "Visible::Shown"]);
    }

    #[test]
    fn markdown_titles_with_selector_characters_are_quoted_segments() {
        let parsed = collect_markdown("# Guide #1\n## Install :: advanced > safe\n");
        assert_eq!(
            parsed.symbols[1].qualified_name,
            "[\"Guide #1\"]::[\"Install :: advanced > safe\"]"
        );
        assert_eq!(
            parsed.symbols[1].legacy_qualified_name,
            "Guide #1 > Install :: advanced > safe"
        );
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn quoted_keys_use_their_source_formats_escape_rules() {
        for language in [Language::Toml, Language::Yaml] {
            assert_eq!(
                decode_quoted_scalar(r#""\U00000041""#, language).as_deref(),
                Some("A")
            );
            assert_eq!(
                decode_quoted_scalar(r#""\\U00000041""#, language).as_deref(),
                Some(r"\U00000041")
            );
            assert_eq!(decode_quoted_scalar(r#""\uD800""#, language), None);
        }
        for (source, expected) in [
            (r#""\x41""#, "A"),
            (r#""\N\_\L\P""#, "\u{85}\u{a0}\u{2028}\u{2029}"),
            ("\"a\\\tb\"", "a\tb"),
            ("'a''b'", "a'b"),
        ] {
            assert_eq!(
                decode_quoted_scalar(source, Language::Yaml).as_deref(),
                Some(expected)
            );
        }
        assert_eq!(
            decode_quoted_scalar("'a\\b'", Language::Toml).as_deref(),
            Some("a\\b")
        );
    }

    #[test]
    fn yaml_flow_folding_preserves_escaped_and_blank_line_breaks() {
        for quote in ['\"', '\''] {
            let scalar = format!("{quote}first\n  second{quote}");
            assert_eq!(
                decode_quoted_scalar(&scalar, Language::Yaml).as_deref(),
                Some("first second")
            );
            let scalar = format!("{quote}first\n\n  second{quote}");
            assert_eq!(
                decode_quoted_scalar(&scalar, Language::Yaml).as_deref(),
                Some("first\nsecond")
            );
        }
        assert_eq!(
            decode_quoted_scalar("\"first\\\n  second\"", Language::Yaml).as_deref(),
            Some("firstsecond")
        );
        assert_eq!(
            decode_quoted_scalar("\"a\\n b\"", Language::Yaml).as_deref(),
            Some("a\n b")
        );
        assert_eq!(
            decode_quoted_scalar("\"a\\ \n b\"", Language::Yaml).as_deref(),
            Some("a  b")
        );
    }

    #[test]
    fn document_text_budget_applies_inside_collection() {
        let key = "k".repeat(4096);
        let values = std::iter::repeat_n("0", 600).collect::<Vec<_>>().join(",");
        for (language, source) in [
            (Language::Json, format!("{{\"{key}\":[{values}]}}")),
            (Language::Jsonc, format!("{{\"{key}\":[{values}]}}")),
            (Language::Yaml, format!("\"{key}\": [{values}]")),
            (Language::Toml, format!("\"{key}\" = [{values}]")),
        ] {
            let mut parser = language.parser(Path::new("fixture")).unwrap();
            let tree = parser.parse(&source, None).unwrap();
            assert!(!tree.root_node().has_error());
            let output = collect(&tree, language, &source);
            assert!(output.truncated);
            assert!(output.symbols.len() < 601);
            assert!(
                output.symbols.iter().map(Symbol::text_bytes).sum::<usize>()
                    <= MAX_SYMBOL_TEXT_BYTES
            );
        }
        let source = format!(
            "# {key}\n{}",
            (0..600)
                .map(|i| format!("## child{i}\n"))
                .collect::<String>()
        );
        let output = collect_markdown(&source);
        assert!(output.truncated);
        assert!(output.symbols.len() < 601);
        assert!(
            output.symbols.iter().map(Symbol::text_bytes).sum::<usize>() <= MAX_SYMBOL_TEXT_BYTES
        );
        assert_eq!(output.symbols[0].end_byte, source.len() - 1);
    }

    #[test]
    fn unsupported_yaml_keys_do_not_invent_scalar_paths_or_hide_incompleteness() {
        for source in [
            "? [a,b]\n: \n  child: 1\nnormal: 2\n",
            "? |\n  first\n  second\n: 1\nnormal: 2\n",
        ] {
            let mut parser = Language::Yaml.parser(Path::new("fixture.yaml")).unwrap();
            let tree = parser.parse(source, None).unwrap();
            assert!(!tree.root_node().has_error());
            let output = collect(&tree, Language::Yaml, source);
            assert!(output.truncated);
            assert_eq!(
                output
                    .symbols
                    .iter()
                    .map(|s| s.qualified_name.as_str())
                    .collect::<Vec<_>>(),
                ["normal"]
            );
        }
    }
}
