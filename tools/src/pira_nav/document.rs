use std::borrow::Cow;
use std::collections::BTreeMap;

use tree_sitter::{Node, Tree};

use crate::language::Language;
use crate::model::Symbol;
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
    match language {
        Language::Json | Language::Jsonc => {
            for child in named_children(tree.root_node()) {
                walk_json_value(child, "", 0, &mut collector);
            }
        }
        Language::Yaml => walk_yaml_stream(tree.root_node(), &mut collector),
        Language::Toml => walk_toml_document(tree.root_node(), &mut collector),
        _ => unreachable!("document collector requires a structured-document language"),
    }
    DocumentSymbols {
        symbols: collector.symbols,
        truncated: collector.truncated,
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
    let mut headings = markdown_headings(source);
    let truncated = headings.len() > MAX_DOCUMENT_SYMBOLS;
    headings.truncate(MAX_DOCUMENT_SYMBOLS.saturating_add(1));
    let mut hierarchy = Vec::<(usize, String)>::new();
    let mut symbols = Vec::with_capacity(headings.len().min(MAX_DOCUMENT_SYMBOLS));
    for (index, heading) in headings.iter().take(MAX_DOCUMENT_SYMBOLS).enumerate() {
        while hierarchy
            .last()
            .is_some_and(|(level, _)| *level >= heading.level)
        {
            hierarchy.pop();
        }
        hierarchy.push((heading.level, heading.title.clone()));
        let qualified_name = hierarchy
            .iter()
            .map(|(_, title)| title.as_str())
            .collect::<Vec<_>>()
            .join(" > ");
        let next_start = headings[index + 1..]
            .iter()
            .find(|candidate| candidate.level <= heading.level)
            .map_or(source.len(), |candidate| candidate.start_byte);
        let end_byte = trim_markdown_section_end(source, heading.heading_end_byte, next_start);
        let (end_row, end_column) = markdown_point(source, end_byte);
        symbols.push(Symbol {
            kind: match heading.level {
                1 => "heading1",
                2 => "heading2",
                3 => "heading3",
                4 => "heading4",
                5 => "heading5",
                _ => "heading6",
            },
            qualified_name,
            signature: heading.title.clone(),
            start_byte: heading.start_byte,
            end_byte,
            start_row: heading.start_row,
            start_column: 0,
            end_row,
            end_column,
            depth: heading.level - 1,
        });
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

fn markdown_point(source: &str, byte: usize) -> (usize, usize) {
    let prefix = &source[..byte.min(source.len())];
    let row = prefix.bytes().filter(|value| *value == b'\n').count();
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix.len(), |(_, line)| line.len());
    (row, column)
}

struct Collector<'a> {
    source: &'a str,
    symbols: Vec<Symbol>,
    truncated: bool,
}

impl<'a> Collector<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            symbols: Vec::new(),
            truncated: false,
        }
    }

    fn push(&mut self, node: Node<'_>, path: String, kind: &'static str, depth: usize) {
        if self.symbols.len() >= MAX_DOCUMENT_SYMBOLS {
            self.truncated = true;
            return;
        }
        self.symbols.push(Symbol {
            kind,
            qualified_name: path,
            signature: document_signature(node, self.source),
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            start_row: node.start_position().row,
            start_column: node.start_position().column,
            end_row: node.end_position().row,
            end_column: node.end_position().column,
            depth,
        });
    }
}

fn walk_json_value(node: Node<'_>, parent: &str, depth: usize, output: &mut Collector<'_>) {
    if output.truncated {
        return;
    }
    match node.kind() {
        "object" => {
            for pair in named_children(node).filter(|child| child.kind() == "pair") {
                let (Some(key_node), Some(value)) = (
                    pair.child_by_field_name("key"),
                    pair.child_by_field_name("value"),
                ) else {
                    continue;
                };
                let Some(key) = json_string(key_node, output.source) else {
                    continue;
                };
                let path = append_key(parent, &key);
                output.push(pair, path.clone(), "key", depth);
                walk_json_value(value, &path, depth + 1, output);
            }
        }
        "array" => {
            for (index, value) in named_children(node)
                .filter(|child| child.kind() != "comment")
                .enumerate()
            {
                let path = append_index(parent, index);
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
        let path = multiple.then(|| format!("document[{index}]"));
        if let Some(path) = &path {
            output.push(document, path.clone(), "document", 0);
        }
        if let Some(value) = yaml_payload(document) {
            walk_yaml_value(
                value,
                path.as_deref().unwrap_or(""),
                usize::from(multiple),
                output,
            );
        }
    }
    collect_yaml_references(root, output);
}

fn walk_yaml_value(node: Node<'_>, parent: &str, depth: usize, output: &mut Collector<'_>) {
    if output.truncated {
        return;
    }
    let node = yaml_payload(node).unwrap_or(node);
    match node.kind() {
        "block_mapping" | "flow_mapping" => {
            for pair in named_children(node)
                .filter(|child| matches!(child.kind(), "block_mapping_pair" | "flow_pair"))
            {
                walk_yaml_pair(pair, parent, depth, output);
            }
        }
        "block_mapping_pair" | "flow_pair" => walk_yaml_pair(node, parent, depth, output),
        "block_sequence" | "flow_sequence" => {
            let values = named_children(node)
                .filter(|child| !matches!(child.kind(), "comment" | "anchor" | "tag"));
            for (index, item) in values.enumerate() {
                let value = if item.kind() == "block_sequence_item" {
                    yaml_payload(item).unwrap_or(item)
                } else {
                    item
                };
                let path = append_index(parent, index);
                output.push(item, path.clone(), "item", depth);
                walk_yaml_value(value, &path, depth + 1, output);
            }
        }
        _ => {}
    }
}

fn walk_yaml_pair(pair: Node<'_>, parent: &str, depth: usize, output: &mut Collector<'_>) {
    let Some(key_node) = pair.child_by_field_name("key") else {
        return;
    };
    let Some(key) = yaml_scalar(key_node, output.source) else {
        return;
    };
    let path = append_key(parent, &key);
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
    if matches!(
        node.kind(),
        "plain_scalar" | "single_quote_scalar" | "double_quote_scalar"
    ) {
        return normalize_quoted_scalar(
            source_slice(source, node.start_byte(), node.end_byte()).as_ref(),
        );
    }
    named_children(node).find_map(|child| yaml_scalar(child, source))
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
        output.push(node, format!("{prefix}{raw}"), node.kind(), 0);
        return;
    }
    for child in named_children(node) {
        collect_yaml_references(child, output);
    }
}

fn walk_toml_document(root: Node<'_>, output: &mut Collector<'_>) {
    let mut table_arrays = BTreeMap::<String, usize>::new();
    for child in named_children(root) {
        match child.kind() {
            "pair" => walk_toml_pair(child, "", 0, output),
            "table" | "table_array_element" => {
                let Some(key_node) = named_children(child).find(|node| is_toml_key(*node)) else {
                    continue;
                };
                let Some(segments) = toml_key_segments(key_node, output.source) else {
                    continue;
                };
                let base = append_segments("", &segments);
                let (path, kind) = if child.kind() == "table_array_element" {
                    let index = table_arrays.entry(base.clone()).or_default();
                    let path = append_index(&base, *index);
                    *index += 1;
                    (path, "table-item")
                } else {
                    (base, "table")
                };
                output.push(child, path.clone(), kind, 0);
                for pair in named_children(child).filter(|node| node.kind() == "pair") {
                    walk_toml_pair(pair, &path, 1, output);
                }
            }
            _ => {}
        }
    }
}

fn walk_toml_pair(pair: Node<'_>, parent: &str, depth: usize, output: &mut Collector<'_>) {
    let mut children = named_children(pair);
    let Some(key_node) = children.find(|node| is_toml_key(*node)) else {
        return;
    };
    let Some(segments) = toml_key_segments(key_node, output.source) else {
        return;
    };
    let path = append_segments(parent, &segments);
    output.push(pair, path.clone(), "key", depth);
    if let Some(value) = named_children(pair).find(|node| !is_toml_key(*node)) {
        walk_toml_value(value, &path, depth + 1, output);
    }
}

fn walk_toml_value(node: Node<'_>, parent: &str, depth: usize, output: &mut Collector<'_>) {
    if output.truncated {
        return;
    }
    match node.kind() {
        "inline_table" => {
            for pair in named_children(node).filter(|child| child.kind() == "pair") {
                walk_toml_pair(pair, parent, depth, output);
            }
        }
        "array" => {
            for (index, value) in named_children(node)
                .filter(|child| child.kind() != "comment")
                .enumerate()
            {
                let path = append_index(parent, index);
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
        "quoted_key" => normalize_quoted_scalar(
            source_slice(source, node.start_byte(), node.end_byte()).as_ref(),
        )
        .map(|key| vec![key]),
        "dotted_key" => {
            let segments = named_children(node)
                .filter_map(|child| toml_key_segments(child, source))
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

fn normalize_quoted_scalar(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() || value.contains(['\n', '\r']) {
        return None;
    }
    if value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str(value)
            .ok()
            .or_else(|| Some(value[1..value.len() - 1].to_owned()));
    }
    if value.starts_with('\'') && value.ends_with('\'') {
        return Some(value[1..value.len() - 1].replace("''", "'"));
    }
    Some(value.to_owned())
}

fn append_segments(parent: &str, segments: &[String]) -> String {
    segments.iter().fold(parent.to_owned(), |path, segment| {
        append_key(&path, segment)
    })
}

fn append_key(parent: &str, key: &str) -> String {
    if key
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-' | '$'))
        && !key.is_empty()
    {
        if parent.is_empty() {
            key.to_owned()
        } else {
            format!("{parent}.{key}")
        }
    } else {
        let quoted = serde_json::to_string(key).expect("serializing a string cannot fail");
        format!("{parent}[{quoted}]")
    }
}

fn append_index(parent: &str, index: usize) -> String {
    format!("{parent}[{index}]")
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
            && next_jsonc_token(bytes, index + 1).is_some_and(|next| matches!(next, b'}' | b']'))
        {
            normalized.get_or_insert_with(|| bytes.to_vec())[index] = b' ';
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
        assert_eq!(append_key("root", "plain-key"), "root.plain-key");
        assert_eq!(append_key("root", "a.b"), "root[\"a.b\"]");
        assert_eq!(append_index("root.items", 2), "root.items[2]");
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
                "Guide > Install",
                "Guide > Install > Verify",
                "Guide > Configuration",
                "Guide > Inspect",
            ]
        );
        let install = &parsed.symbols[1];
        let section = &source[install.start_byte..install.end_byte];
        assert!(section.contains("### Verify"));
        assert!(!section.contains("Configuration"));
        assert!(!parsed.truncated);
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
        assert_eq!(names, ["Visible", "Visible > Shown"]);
    }
}
