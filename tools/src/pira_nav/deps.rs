use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use tree_sitter::Node;

use crate::command::{CommandError, input_error, lsp_error};
use crate::language::Language;
use crate::lsp::{LspService, file_path_from_uri};
use crate::model::ImportEdge;
use crate::parse::{ParsedSyntax, parse_syntax};
use crate::util::{
    absolute_lexical, display_path, one_line, read_source, reject_symlink_components,
};

const MAX_IMPORT_REFERENCES: usize = 10_000;

struct ImportReference {
    line: usize,
    text: String,
    position: Option<(usize, usize)>,
    unsupported: &'static str,
}

/// Resolve syntax import references through a capable server, never filename guesses.
/// The returned edges are not a complete build/runtime dependency graph.
pub fn imports_from_path(
    path: &Path,
    language: Language,
    root: &Path,
    server_root: &Path,
    service: &mut LspService,
) -> Result<Vec<ImportEdge>, CommandError> {
    if language.is_document() {
        return Err(input_error("documents do not support import semantics"));
    }
    reject_symlink_components(path).map_err(input_error)?;
    if !path.starts_with(root) || !path.starts_with(server_root) {
        return Err(input_error(
            "import source must be inside the dependency root and --lsp-root",
        ));
    }
    let (source, references) = extract_imports(path, language).map_err(input_error)?;
    service.require_definitions(language).map_err(lsp_error)?;
    let mut output = Vec::with_capacity(references.len());
    for reference in references {
        let mut edge = ImportEdge {
            source: path.to_path_buf(),
            line: reference.line,
            text: reference.text,
            target: None,
            target_label: reference.unsupported.into(),
            resolution: "unsupported",
        };
        if let Some((row, column)) = reference.position {
            let locations = service
                .definition(path, language, &source, row, column)
                .map_err(lsp_error)?;
            let mut targets = BTreeSet::new();
            let mut unsupported = false;
            let mut blocked = false;
            for location in locations {
                let Some(target) = file_path_from_uri(&location.uri).map_err(lsp_error)? else {
                    unsupported = true;
                    continue;
                };
                let target = absolute_lexical(&target, root);
                if !target.starts_with(root) || reject_symlink_components(&target).is_err() {
                    blocked = true;
                } else if !target.is_file() || target == path {
                    // A server can return an import alias itself, a directory or a generated URI.
                    // None establishes a distinct file dependency.
                    unsupported = true;
                } else {
                    targets.insert(target);
                }
            }
            let (resolution, label) = if blocked {
                ("blocked", "outside-root-or-symlink")
            } else if unsupported {
                ("unsupported", "non-file-missing-or-self-definition")
            } else if targets.len() > 1 {
                ("ambiguous", "multiple-definition-files")
            } else if let Some(target) = targets.into_iter().next() {
                edge.target_label = display_path(&target, root);
                edge.target = Some(target);
                ("lsp", "")
            } else {
                ("unresolved", "no-definition")
            };
            edge.resolution = resolution;
            if edge.target.is_none() {
                edge.target_label = label.into();
            }
        }
        output.push(edge);
    }
    Ok(output)
}

fn extract_imports(
    path: &Path,
    language: Language,
) -> Result<(String, Vec<ImportReference>), String> {
    if language == Language::Lean {
        let source = read_source(path)?;
        let references = lean_imports_from_source(&source)?;
        return Ok((source, references));
    }
    let parsed = parse_syntax(path, language)?;
    let mut output = Vec::new();
    collect(parsed.tree.root_node(), &parsed, &mut output)?;
    Ok((parsed.source, output))
}

fn check_reference_limit(output: &[ImportReference]) -> Result<(), String> {
    if output.len() > MAX_IMPORT_REFERENCES {
        Err(format!(
            "import reference inventory exceeds {MAX_IMPORT_REFERENCES}; narrow the source file"
        ))
    } else {
        Ok(())
    }
}

fn text<'a>(node: Node<'_>, parsed: &'a ParsedSyntax) -> &'a str {
    &parsed.source[node.byte_range()]
}

fn push_reference(
    statement: Node<'_>,
    position: Option<(usize, usize)>,
    reason: &'static str,
    parsed: &ParsedSyntax,
    output: &mut Vec<ImportReference>,
) -> Result<(), String> {
    output.push(ImportReference {
        line: statement.start_position().row + 1,
        text: import_text(text(statement, parsed)),
        position,
        unsupported: reason,
    });
    check_reference_limit(output)
}

fn import_text(source: &str) -> String {
    let mut end = source.len().min(1024);
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = one_line(&source[..end]);
    if end < source.len() {
        result.push('…');
    }
    result
}

fn point(node: Node<'_>) -> (usize, usize) {
    (node.start_position().row, node.start_position().column)
}

fn literal_reference(
    statement: Node<'_>,
    value: Option<Node<'_>>,
    parsed: &ParsedSyntax,
    output: &mut Vec<ImportReference>,
) -> Result<(), String> {
    let Some(value) = value else {
        return push_reference(statement, None, "missing-import-operand", parsed, output);
    };
    // PIRA: only a direct literal operand is eligible. An arbitrary descendant string
    // in a concatenation/call/interpolation does not identify its enclosing expression.
    let raw = text(value, parsed);
    let is_literal = matches!(
        value.kind(),
        "string"
            | "string_literal"
            | "system_lib_string"
            | "interpreted_string_literal"
            | "raw_string_literal"
            | "string_lit"
            | "raw_string"
    );
    let mut cursor = value.walk();
    let dynamic = value.named_children(&mut cursor).any(|child| {
        matches!(
            child.kind(),
            "interpolation"
                | "interpolation_expression"
                | "string_interpolation"
                | "expansion"
                | "simple_expansion"
                | "command_substitution"
                | "template_substitution"
        )
    });
    if !is_literal || dynamic || raw.len() < 3 {
        return push_reference(
            statement,
            None,
            "dynamic-or-unsupported-import-operand",
            parsed,
            output,
        );
    }
    let raw_offset = usize::from(parsed.language == Language::Dart && raw.starts_with(['r', 'R']));
    let literal = &raw[raw_offset..];
    let quote_bytes = if literal.starts_with("\"\"\"") || literal.starts_with("'''") {
        3
    } else if literal.starts_with(['\"', '\'', '`', '<']) {
        1
    } else {
        return push_reference(
            statement,
            None,
            "literal-delimiter-unsupported",
            parsed,
            output,
        );
    };
    let (row, col) = point(value);
    push_reference(
        statement,
        Some((row, col + raw_offset + quote_bytes)),
        "",
        parsed,
        output,
    )
}

// Walk syntax reference tokens, not text split on punctuation. Servers alone assign identity.
// Aliases/modifiers/comments are not references; wildcards remain explicitly unexpanded.
fn named_references(
    statement: Node<'_>,
    node: Node<'_>,
    parsed: &ParsedSyntax,
    output: &mut Vec<ImportReference>,
) -> Result<(), String> {
    let kind = node.kind();
    if kind.contains("comment")
        || matches!(
            kind,
            "visibility_modifier" | "modifiers" | "annotation" | "attribute_item"
        )
    {
        return Ok(());
    }
    if matches!(
        kind,
        "wildcard_import" | "use_wildcard" | "asterisk" | "namespace_wildcard"
    ) {
        return push_reference(
            statement,
            None,
            "wildcard-members-not-enumerated",
            parsed,
            output,
        );
    }
    if matches!(kind, "aliased_import" | "use_as_clause")
        && let Some(original) = node
            .child_by_field_name("name")
            .or_else(|| node.child_by_field_name("path"))
    {
        return named_references(statement, original, parsed, output);
    }
    if kind == "import_alias" && parsed.language == Language::Kotlin {
        return Ok(());
    }
    if matches!(kind, "import_alias" | "as_renamed_identifier")
        && let Some(original) = node.named_child(0)
    {
        return named_references(statement, original, parsed, output);
    }
    if matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "property_identifier"
            | "simple_identifier"
            | "name"
            | "operator_identifier"
            | "self"
            | "crate"
            | "super"
    ) && node.named_child_count() == 0
    {
        return push_reference(statement, Some(point(node)), "", parsed, output);
    }
    let mut cursor = node.walk();
    for (index, child) in node.children(&mut cursor).enumerate() {
        let field = node.field_name_for_child(index as u32);
        if field == Some("alias")
            || (parsed.language == Language::CSharp
                && kind == "using_directive"
                && field == Some("name"))
        {
            continue;
        }
        if child.is_named() {
            named_references(statement, child, parsed, output)?;
        }
    }
    Ok(())
}

fn first_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn collect(
    node: Node<'_>,
    parsed: &ParsedSyntax,
    output: &mut Vec<ImportReference>,
) -> Result<(), String> {
    let before = output.len();
    let kind = node.kind();
    let language = parsed.language;
    let mut recognized = true;
    match language {
        Language::Python
            if matches!(
                kind,
                "import_statement" | "import_from_statement" | "future_import_statement"
            ) =>
        {
            named_references(node, node, parsed, output)?;
        }
        Language::Rust if kind == "use_declaration" => {
            if let Some(argument) = node.child_by_field_name("argument") {
                named_references(node, argument, parsed, output)?;
            }
        }
        Language::Rust if kind == "extern_crate_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                push_reference(node, Some(point(name)), "", parsed, output)?;
            }
        }
        Language::Rust if kind == "mod_item" && node.child_by_field_name("body").is_none() => {
            if let Some(name) = node.child_by_field_name("name") {
                push_reference(node, Some(point(name)), "", parsed, output)?;
            }
        }
        Language::JavaScript | Language::TypeScript
            if matches!(kind, "import_statement" | "export_statement")
                && node.child_by_field_name("source").is_some() =>
        {
            literal_reference(node, node.child_by_field_name("source"), parsed, output)?;
        }
        Language::JavaScript | Language::TypeScript if kind == "import_statement" => {
            let source = first_child(node, "import_require_clause")
                .and_then(|clause| clause.child_by_field_name("source"));
            literal_reference(node, source, parsed, output)?;
        }
        Language::TypeScript if kind == "import_alias" => {
            if let Some(original) = node.named_child(1) {
                named_references(node, original, parsed, output)?;
            }
        }
        Language::JavaScript | Language::TypeScript
            if kind == "call_expression"
                && node
                    .child_by_field_name("function")
                    .is_some_and(|name| matches!(text(name, parsed), "require" | "import")) =>
        {
            let argument = node
                .child_by_field_name("arguments")
                .and_then(|args| args.named_child(0));
            literal_reference(node, argument, parsed, output)?;
        }
        Language::C | Language::Cpp | Language::Cuda if kind == "preproc_include" => {
            literal_reference(node, node.child_by_field_name("path"), parsed, output)?;
        }
        Language::Go if kind == "import_spec" => {
            literal_reference(node, node.child_by_field_name("path"), parsed, output)?;
        }
        Language::Bash
            if kind == "command"
                && node
                    .child_by_field_name("name")
                    .is_some_and(|name| matches!(text(name, parsed), "source" | ".")) =>
        {
            let argument = node.child_by_field_name("argument");
            if let Some(value) = argument
                .filter(|value| bash_literal(text(*value, parsed)).is_some_and(|v| !v.is_empty()))
            {
                let (row, col) = point(value);
                let quote = usize::from(text(value, parsed).starts_with(['\'', '"']));
                push_reference(node, Some((row, col + quote)), "", parsed, output)?;
            } else {
                push_reference(node, None, "dynamic-shell-path", parsed, output)?;
            }
        }
        _ if matches!(
            (language, kind),
            (
                Language::Java | Language::Swift | Language::Scala,
                "import_declaration"
            ) | (Language::Kotlin, "import_header")
                | (Language::CSharp, "using_directive")
                | (Language::Php, "namespace_use_declaration")
                | (Language::Julia, "using_statement" | "import_statement")
        ) =>
        {
            named_references(node, node, parsed, output)?;
        }
        Language::Lua
            if kind == "function_call"
                && node
                    .child_by_field_name("name")
                    .is_some_and(|name| text(name, parsed) == "require") =>
        {
            literal_reference(
                node,
                node.child_by_field_name("arguments")
                    .and_then(|args| args.named_child(0)),
                parsed,
                output,
            )?;
        }
        Language::Php
            if matches!(
                kind,
                "include_expression"
                    | "include_once_expression"
                    | "require_expression"
                    | "require_once_expression"
            ) =>
        {
            literal_reference(node, node.named_child(0), parsed, output)?;
        }
        Language::Ruby
            if kind == "call"
                && node.child_by_field_name("method").is_some_and(|name| {
                    matches!(text(name, parsed), "require" | "require_relative" | "load")
                }) =>
        {
            literal_reference(
                node,
                first_child(node, "argument_list").and_then(|args| args.named_child(0)),
                parsed,
                output,
            )?;
        }
        Language::R
            if kind == "call"
                && node.child_by_field_name("function").is_some_and(|name| {
                    matches!(
                        text(name, parsed),
                        "source" | "sys.source" | "library" | "require"
                    )
                }) =>
        {
            let function = node
                .child_by_field_name("function")
                .expect("matched function");
            if matches!(text(function, parsed), "source" | "sys.source") {
                let value = node.child_by_field_name("arguments").and_then(|args| {
                    let mut cursor = args.walk();
                    let arguments = args
                        .children_by_field_name("argument", &mut cursor)
                        .collect::<Vec<_>>();
                    let file = arguments
                        .iter()
                        .find(|arg| {
                            arg.child_by_field_name("name")
                                .is_some_and(|name| text(name, parsed) == "file")
                        })
                        .or_else(|| {
                            arguments
                                .iter()
                                .find(|arg| arg.child_by_field_name("name").is_none())
                        });
                    file.and_then(|arg| arg.child_by_field_name("value"))
                });
                literal_reference(node, value, parsed, output)?;
            } else {
                push_reference(
                    node,
                    None,
                    "unevaluated-package-argument-unsupported",
                    parsed,
                    output,
                )?;
            }
        }
        Language::Elixir
            if kind == "call"
                && node.child_by_field_name("target").is_some_and(|name| {
                    matches!(text(name, parsed), "alias" | "import" | "require" | "use")
                }) =>
        {
            if let Some(module) = first_child(node, "arguments")
                .and_then(|args| args.named_child(0))
                .filter(|module| module.kind() == "alias")
            {
                push_reference(node, Some(point(module)), "", parsed, output)?;
            } else {
                push_reference(
                    node,
                    None,
                    "macro-module-expansion-unsupported",
                    parsed,
                    output,
                )?;
            }
        }
        Language::Julia
            if kind == "call_expression"
                && node
                    .named_child(0)
                    .is_some_and(|name| text(name, parsed) == "include") =>
        {
            literal_reference(
                node,
                first_child(node, "argument_list").and_then(|args| args.named_child(0)),
                parsed,
                output,
            )?;
        }
        Language::Dart
            if matches!(kind, "library_import" | "library_export" | "part_directive") =>
        {
            collect_dart_uris(node, node, parsed, output)?;
        }
        Language::PowerShell
            if kind == "command"
                && (node
                    .child_by_field_name("command_name")
                    .is_some_and(|name| {
                        text(name, parsed).eq_ignore_ascii_case("import-module")
                    })
                    || first_child(node, "command_invokation_operator")
                        .is_some_and(|op| text(op, parsed) == ".")) =>
        {
            push_reference(
                node,
                None,
                "shell-module-semantics-unsupported",
                parsed,
                output,
            )?;
        }
        Language::Hcl
            if kind == "block"
                && first_child(node, "identifier")
                    .is_some_and(|name| text(name, parsed) == "module") =>
        {
            push_reference(
                node,
                None,
                "module-directory-membership-unsupported",
                parsed,
                output,
            )?;
        }
        _ => recognized = false,
    }
    if recognized {
        if output.len() == before {
            push_reference(
                node,
                None,
                "import-reference-syntax-unsupported",
                parsed,
                output,
            )?;
        }
        return Ok(());
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, parsed, output)?;
    }
    Ok(())
}

fn collect_dart_uris(
    statement: Node<'_>,
    node: Node<'_>,
    parsed: &ParsedSyntax,
    output: &mut Vec<ImportReference>,
) -> Result<(), String> {
    if node.kind() == "uri" {
        return literal_reference(statement, node.named_child(0), parsed, output);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_dart_uris(statement, child, parsed, output)?;
    }
    Ok(())
}

fn bash_literal(argument: &str) -> Option<String> {
    let mut result = String::new();
    let mut quote = None;
    let mut chars = argument.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => result.push(ch),
            (None, '\'' | '"') => quote = Some(ch),
            (_, '\\') => {
                let next = chars.next()?;
                if next == '\n' {
                    continue;
                }
                if quote == Some('"') && !matches!(next, '$' | '`' | '"' | '\\') {
                    result.push('\\');
                }
                result.push(next);
            }
            (_, '$' | '`') => return None,
            (None, '~' | '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '<' | '>') => return None,
            (None, ch) if ch.is_whitespace() => return None,
            _ => result.push(ch),
        }
    }
    quote.is_none().then_some(result)
}

#[derive(Clone, Copy)]
struct LeanHeaderToken {
    start: usize,
    end: usize,
    line: usize,
}

struct LeanHeaderScanner<'a> {
    source: &'a str,
    offset: usize,
    line: usize,
}

impl<'a> LeanHeaderScanner<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            offset: 0,
            line: 1,
        }
    }

    fn checkpoint(&self) -> (usize, usize) {
        (self.offset, self.line)
    }

    fn restore(&mut self, checkpoint: (usize, usize)) {
        (self.offset, self.line) = checkpoint;
    }

    fn token_text(&self, token: LeanHeaderToken) -> &'a str {
        &self.source[token.start..token.end]
    }

    fn next_token(&mut self) -> Result<Option<LeanHeaderToken>, String> {
        self.skip_trivia()?;
        if self.offset == self.source.len() {
            return Ok(None);
        }
        let start = self.offset;
        let line = self.line;
        let mut quoted = false;
        while self.offset < self.source.len() {
            if !quoted
                && (self.current_char().is_some_and(char::is_whitespace)
                    || self.remaining().starts_with("--")
                    || self.remaining().starts_with("/-"))
            {
                break;
            }
            let character = self.current_char().expect("offset is within source");
            quoted = match character {
                '«' if !quoted => true,
                '»' if quoted => false,
                _ => quoted,
            };
            self.advance_char(character);
        }
        if quoted {
            return Err(format!(
                "unterminated quoted identifier in Lean module header at line {line}"
            ));
        }
        Ok(Some(LeanHeaderToken {
            start,
            end: self.offset,
            line,
        }))
    }

    fn skip_trivia(&mut self) -> Result<(), String> {
        loop {
            while let Some(character) = self.current_char().filter(|value| value.is_whitespace()) {
                self.advance_char(character);
            }
            if self.remaining().starts_with("--") {
                while let Some(character) = self.current_char() {
                    self.advance_char(character);
                    if character == '\n' {
                        break;
                    }
                }
                continue;
            }
            if self.remaining().starts_with("/-") {
                let comment_line = self.line;
                self.offset += 2;
                let mut depth = 1usize;
                while self.offset < self.source.len() {
                    if self.remaining().starts_with("/-") {
                        depth = depth.saturating_add(1);
                        self.offset += 2;
                    } else if self.remaining().starts_with("-/") {
                        depth -= 1;
                        self.offset += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        let character = self.current_char().expect("offset is within source");
                        self.advance_char(character);
                    }
                }
                if depth != 0 {
                    return Err(format!(
                        "unterminated block comment in Lean module header at line {comment_line}"
                    ));
                }
                continue;
            }
            return Ok(());
        }
    }

    fn current_char(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    fn remaining(&self) -> &'a str {
        &self.source[self.offset..]
    }

    fn advance_char(&mut self, character: char) {
        self.offset += character.len_utf8();
        self.line += usize::from(character == '\n');
    }
}

fn lean_imports_from_source(source: &str) -> Result<Vec<ImportReference>, String> {
    let mut scanner = LeanHeaderScanner::new(source);
    consume_lean_header_keyword(&mut scanner, "module")?;
    consume_lean_header_keyword(&mut scanner, "prelude")?;

    let mut output = Vec::new();
    loop {
        let checkpoint = scanner.checkpoint();
        let Some(first) = scanner.next_token()? else {
            break;
        };
        let start = first.start;
        let line = first.line;
        let mut keyword = scanner.token_text(first);
        if keyword == "public" {
            let Some(token) = scanner.next_token()? else {
                scanner.restore(checkpoint);
                break;
            };
            keyword = scanner.token_text(token);
        }
        if keyword == "meta" {
            let Some(token) = scanner.next_token()? else {
                scanner.restore(checkpoint);
                break;
            };
            keyword = scanner.token_text(token);
        }
        if keyword != "import" {
            scanner.restore(checkpoint);
            break;
        }

        let Some(mut module_token) = scanner.next_token()? else {
            return Err(format!("Lean import at line {line} has no module name"));
        };
        if scanner.token_text(module_token) == "all" {
            module_token = scanner
                .next_token()?
                .ok_or_else(|| format!("Lean import at line {line} has no module name"))?;
        }
        let module = scanner.token_text(module_token);
        lean_module_path(module)
            .ok_or_else(|| format!("invalid Lean module name {module:?} at line {line}"))?;
        let row_start = source[..module_token.start]
            .rfind('\n')
            .map_or(0, |at| at + 1);
        output.push(ImportReference {
            line,
            text: import_text(&source[start..module_token.end]),
            position: Some((module_token.line - 1, module_token.start - row_start)),
            unsupported: "",
        });
        check_reference_limit(&output)?;
    }
    Ok(output)
}

fn consume_lean_header_keyword(
    scanner: &mut LeanHeaderScanner<'_>,
    expected: &str,
) -> Result<(), String> {
    let checkpoint = scanner.checkpoint();
    if scanner
        .next_token()?
        .is_some_and(|token| scanner.token_text(token) == expected)
    {
        return Ok(());
    }
    scanner.restore(checkpoint);
    Ok(())
}

fn lean_module_path(module: &str) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    let mut segment = String::new();
    let mut quoted = false;
    for character in module.chars() {
        match character {
            '«' => {
                if quoted {
                    return None;
                }
                quoted = true;
            }
            '»' => {
                if !quoted {
                    return None;
                }
                quoted = false;
            }
            '.' if !quoted => {
                if !push_lean_module_segment(&mut relative, &mut segment) {
                    return None;
                }
            }
            '/' | '\\' | '\0' => return None,
            _ => segment.push(character),
        }
    }
    if quoted || !push_lean_module_segment(&mut relative, &mut segment) {
        return None;
    }
    let mut file_name = relative.file_name()?.to_os_string();
    file_name.push(".lean");
    relative.set_file_name(file_name);
    Some(relative)
}

fn push_lean_module_segment(relative: &mut PathBuf, segment: &mut String) -> bool {
    if segment.is_empty() || segment == "." || segment == ".." {
        return false;
    }
    let mut components = Path::new(segment.as_str()).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return false;
    }
    relative.push(std::mem::take(segment));
    true
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{extract_imports, lean_module_path};
    use crate::language::Language;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn bundled_import_syntax_retains_references_or_explicit_unsupported_occurrences() {
        let root = std::env::temp_dir().join(format!(
            "nav-import-syntax-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        // Expectations derive from grammar operand roles, not the old guessed target paths.
        for (language, suffix, source, resolves) in [
            (Language::Python, "py", "from x import y as z\n", true),
            (
                Language::Rust,
                "rs",
                "pub(crate) use x::{y as z, a};\n",
                true,
            ),
            (Language::JavaScript, "js", "import x from './x';\n", true),
            (
                Language::TypeScript,
                "ts",
                "import x = require('./x');\n",
                true,
            ),
            (Language::C, "c", "#include \"a.h\"\n", true),
            (Language::Cpp, "cpp", "#include <a.h>\n", true),
            (Language::Cuda, "cu", "#include \"a.h\"\n", true),
            (Language::Go, "go", "package main\nimport \"x\"\n", true),
            (Language::Java, "java", "import x.Y;\n", true),
            (Language::Kotlin, "kt", "import x.Y as Z\n", true),
            (Language::Swift, "swift", "import X\n", true),
            (Language::Scala, "scala", "import x.Y\n", true),
            (Language::CSharp, "cs", "using X.Y;\n", true),
            (Language::Php, "php", "<?php use X\\Y as Z;\n", true),
            (Language::Php, "php", "<?php require 'x.php';\n", true),
            (Language::Lua, "lua", "require('x')\n", true),
            (Language::Bash, "sh", "source \"x.sh\"\n", true),
            (Language::Ruby, "rb", "require_relative 'x'\n", true),
            (Language::R, "R", "source('x.R')\n", true),
            (Language::R, "R", "library(x)\n", false),
            (Language::Elixir, "ex", "alias X.Y\n", true),
            (Language::Julia, "jl", "using X\n", true),
            (Language::Julia, "jl", "include(\"x.jl\")\n", true),
            (Language::Dart, "dart", "import 'x.dart';\n", true),
            (Language::PowerShell, "ps1", "Import-Module x\n", false),
            (
                Language::Hcl,
                "tf",
                "module \"x\" { source = \"./x\" }\n",
                false,
            ),
        ] {
            let path = root.join(format!("sample.{suffix}"));
            fs::write(&path, source).unwrap();
            let (_, references) = extract_imports(&path, language)
                .unwrap_or_else(|e| panic!("{language:?} {source:?}: {e}"));
            assert!(
                !references.is_empty(),
                "{language:?}: import disappeared: {source:?}"
            );
            assert_eq!(
                references.iter().any(|r| r.position.is_some()),
                resolves,
                "{language:?}: {source:?}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn csharp_resource_statements_are_not_imports() {
        let root = std::env::temp_dir().join(format!(
            "nav-import-roles-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sample.cs");
        fs::write(
            &path,
            "class C { void F() { using (var x = Get()) { Work(); } } }",
        )
        .unwrap();
        assert!(
            extract_imports(&path, Language::CSharp)
                .unwrap()
                .1
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn r_source_queries_file_not_other_named_string_options() {
        let root = std::env::temp_dir().join(format!(
            "nav-r-import-roles-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("sample.R");
        fs::write(&path, "source(encoding='UTF-8', file='x.R')\n").unwrap();
        let (source, references) = extract_imports(&path, Language::R).unwrap();
        assert_eq!(references.len(), 1);
        let (_, column) = references[0].position.unwrap();
        assert!(
            source[column..].starts_with("x.R"),
            "queried wrong operand: {}",
            &source[column..]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dynamic_operands_never_resolve_arbitrary_descendant_literals() {
        let root = std::env::temp_dir().join(format!(
            "nav-import-dynamic-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        for (language, suffix, source) in [
            (Language::JavaScript, "js", "require(prefix + './x');\n"),
            (Language::Php, "php", "<?php require $prefix . 'x.php';\n"),
            (Language::Lua, "lua", "require(prefix .. 'x')\n"),
            (Language::Ruby, "rb", "require_relative \"#{prefix}/x\"\n"),
            (Language::Bash, "sh", "source \"$BASE/x\"\n"),
            (Language::C, "c", "#include HEADER\n"),
            (Language::R, "R", "source(paste0(prefix, 'x.R'))\n"),
        ] {
            let path = root.join(format!("sample.{suffix}"));
            fs::write(&path, source).unwrap();
            let (_, references) = extract_imports(&path, language).unwrap();
            assert_eq!(references.len(), 1, "{language:?}: {source:?}");
            assert!(references[0].position.is_none(), "{language:?}: {source:?}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lean_imports_ignore_dynamic_and_deep_body_syntax() {
        let root = std::env::temp_dir().join(format!(
            "pira-nav-lean-imports-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("Demo")).unwrap();
        fs::write(root.join("Demo/Base.lean"), "def base : Nat := 1\n").unwrap();
        let source = root.join("Deep.lean");
        fs::write(
            &source,
            format!(
                "import Demo.Base\n\nsyntax \"pira_custom\" : term\ndef deep : Nat := {}0{}\n",
                "(".repeat(300),
                ")".repeat(300)
            ),
        )
        .unwrap();

        let (_, edges) = extract_imports(&source, Language::Lean).unwrap();
        assert_eq!(edges.len(), 1);
        assert!(edges[0].position.is_some());
        assert_eq!(edges[0].position, Some((0, 7)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn lean_header_scanner_handles_comments_modifiers_and_quoted_names() {
        let root = std::env::temp_dir().join(format!(
            "pira-nav-lean-header-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("Demo")).unwrap();
        fs::write(root.join("Demo/Base.lean"), "def base : Nat := 1\n").unwrap();
        fs::write(
            root.join("Demo/Quoted Name.lean"),
            "def quoted : Nat := 2\n",
        )
        .unwrap();
        let source = root.join("Header.lean");
        fs::write(
            &source,
            "/- outer /- import Ignored.Nested -/ comment -/\n\
             module -- module annotation\n\
             prelude\n\
             public meta import all Demo.Base -- retained comment\n\
             import Demo.«Quoted Name»\n\
             /-! import Ignored.Doc -/\n\
             deprecated_module \"import Ignored.Deprecation instead\" (since := \"2026-01-01\")\n\
             def body : String := \"import Ignored.String\"\n",
        )
        .unwrap();

        let (_, edges) = extract_imports(&source, Language::Lean).unwrap();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].line, 4);
        assert_eq!(edges[0].text, "public meta import all Demo.Base");
        assert!(edges[0].position.is_some());
        assert_eq!(edges[1].line, 5);
        assert_eq!(edges[1].text, "import Demo.«Quoted Name»");
        assert_eq!(edges[1].position, Some((4, 7)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn lean_header_scanner_rejects_incomplete_header_trivia() {
        let root = std::env::temp_dir().join(format!(
            "pira-nav-lean-malformed-header-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("Malformed.lean");
        fs::write(&source, "/- import Demo.Base\n").unwrap();

        let error = extract_imports(&source, Language::Lean).err().unwrap();
        assert!(error.contains("unterminated block comment"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn lean_module_paths_are_relative_and_quote_aware() {
        assert_eq!(
            lean_module_path("Demo.«Quoted.Name»"),
            Some(PathBuf::from("Demo/Quoted.Name.lean"))
        );
        for invalid in [
            "",
            ".Demo",
            "Demo.",
            "Demo..Base",
            "Demo/Injected",
            "Demo\\Injected",
            "Demo.«Unclosed",
            "Demo.Unopened»",
            "Demo.««Nested»»",
        ] {
            assert_eq!(lean_module_path(invalid), None, "accepted {invalid:?}");
        }
    }
}
