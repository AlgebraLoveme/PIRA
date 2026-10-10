use std::path::{Path, PathBuf};

use tree_sitter::{Node, Point, Tree};

use crate::document;
use crate::language::Language;
use crate::model::{MAX_SYMBOL_TEXT_BYTES, ParseBackend, Symbol, SymbolPath};
use crate::util::{hash16, one_line, percent_encode, read_source, source_slice};

const MAX_SYNTAX_DEPTH: usize = 256;
pub const MAX_CODE_SYMBOLS: usize = 20_000;
const MAX_CODE_SYMBOL_TEXT_BYTES: usize = MAX_SYMBOL_TEXT_BYTES;

pub struct ParsedFile {
    pub path: PathBuf,
    pub language: Language,
    pub source: String,
    pub symbols: Vec<Symbol>,
    pub backend: ParseBackend,
    pub syntax_defects: usize,
    pub symbols_truncated: bool,
}

pub struct ParsedSyntax {
    pub language: Language,
    pub source: String,
    pub tree: Tree,
}

impl ParsedFile {
    pub fn selector(&self, symbol: &Symbol, shown_path: &str) -> String {
        let bytes = self
            .source
            .get(symbol.start_byte..symbol.end_byte)
            .unwrap_or_default()
            .as_bytes();
        format!(
            "pira://{}/{}#{}/{}@{}",
            self.language.name(),
            percent_encode(shown_path),
            percent_encode(symbol.kind),
            percent_encode(&symbol.qualified_name),
            hash16(bytes)
        )
    }
}

pub fn parse_file(path: &Path, language: Language) -> Result<ParsedFile, String> {
    parse_file_source(path, language, read_source(path)?)
}

pub fn parse_file_source(
    path: &Path,
    language: Language,
    source: String,
) -> Result<ParsedFile, String> {
    let (symbols, syntax_defects, symbols_truncated) =
        parse_source_symbols_state(path, language, &source)?;
    Ok(ParsedFile {
        path: path.to_path_buf(),
        language,
        source,
        symbols,
        backend: ParseBackend::Native,
        syntax_defects,
        symbols_truncated,
    })
}

pub fn parse_source_symbols(
    path: &Path,
    language: Language,
    source: &str,
) -> Result<(Vec<Symbol>, usize), String> {
    let (symbols, defects, _) = parse_source_symbols_state(path, language, source)?;
    Ok((symbols, defects))
}

fn parse_source_symbols_state(
    path: &Path,
    language: Language,
    source: &str,
) -> Result<(Vec<Symbol>, usize, bool), String> {
    if language == Language::Markdown {
        let collected = document::collect_markdown(source);
        return Ok((collected.symbols, 0, collected.truncated));
    }
    let mut parser = language.parser(path)?;
    let input = document::parse_input(language, source);
    let tree = parser
        .parse(input.as_bytes(), None)
        .ok_or_else(|| format!("{} parser returned no tree", language.name()))?;
    let defects = match inspect_tree(tree.root_node()) {
        Ok(defects) => defects,
        Err(_) if language == Language::Lean => return Ok((Vec::new(), 1, false)),
        Err(depth) => {
            return Err(format!(
                "syntax tree nesting exceeds supported depth of {MAX_SYNTAX_DEPTH} in {} (observed at least {depth})",
                path.display()
            ));
        }
    };
    if defects > 0 {
        return Ok((Vec::new(), defects, false));
    }
    let (mut symbols, mut symbols_truncated) = collect_symbols(&tree, language, source);
    symbols.sort_by_key(|symbol| (symbol.start_byte, symbol.end_byte));
    symbols.dedup_by(|left, right| {
        left.start_byte == right.start_byte
            && left.end_byte == right.end_byte
            && left.kind == right.kind
            && left.qualified_name == right.qualified_name
    });
    let mut text_bytes = 0usize;
    let keep = symbols
        .iter()
        .take(MAX_CODE_SYMBOLS)
        .take_while(|symbol| {
            let next = text_bytes.saturating_add(symbol.text_bytes());
            if next > MAX_CODE_SYMBOL_TEXT_BYTES {
                false
            } else {
                text_bytes = next;
                true
            }
        })
        .count();
    symbols_truncated |= keep < symbols.len();
    symbols.truncate(keep);
    Ok((symbols, defects, symbols_truncated))
}

pub fn parse_syntax(path: &Path, language: Language) -> Result<ParsedSyntax, String> {
    let (syntax, defects) = parse_native(path, language)?;
    if defects > 0 {
        return Err(format!(
            "native parser found {defects} syntax defect(s); imports and dependency commands require clean code"
        ));
    }
    Ok(syntax)
}

fn parse_native(path: &Path, language: Language) -> Result<(ParsedSyntax, usize), String> {
    let source = read_source(path)?;
    let mut parser = language.parser(path)?;
    let input = document::parse_input(language, &source);
    let tree = parser
        .parse(input.as_bytes(), None)
        .ok_or_else(|| format!("{} parser returned no tree", language.name()))?;
    let defects = inspect_tree(tree.root_node()).map_err(|depth| {
        format!(
            "syntax tree nesting exceeds supported depth of {MAX_SYNTAX_DEPTH} in {} (observed at least {depth})",
            path.display()
        )
    })?;
    Ok((
        ParsedSyntax {
            language,
            source,
            tree,
        },
        defects,
    ))
}

fn collect_symbols(tree: &Tree, language: Language, source: &str) -> (Vec<Symbol>, bool) {
    if language.is_document() {
        let collected = document::collect(tree, language, source);
        return (collected.symbols, collected.truncated);
    }
    let mut symbols = SymbolCollector::default();
    match language {
        Language::Python => walk_python(tree.root_node(), source, None, false, 0, &mut symbols),
        Language::Rust => walk_rust(tree.root_node(), source, None, 0, &mut symbols),
        Language::Java => walk_java(tree.root_node(), source, None, 0, &mut symbols),
        Language::C => walk_c_family(tree.root_node(), source, None, 0, false, &mut symbols),
        Language::Cpp => walk_c_family(tree.root_node(), source, None, 0, true, &mut symbols),
        Language::Cuda => walk_c_family(tree.root_node(), source, None, 0, true, &mut symbols),
        Language::Bash => walk_bash(tree.root_node(), source, None, 0, &mut symbols),
        Language::Go => walk_go(tree.root_node(), source, None, 0, &mut symbols),
        Language::JavaScript => {
            walk_ecmascript(tree.root_node(), source, None, 0, false, &mut symbols)
        }
        Language::TypeScript => {
            walk_ecmascript(tree.root_node(), source, None, 0, true, &mut symbols)
        }
        Language::CSharp => walk_csharp_root(tree.root_node(), source, &mut symbols),
        Language::PowerShell => walk_powershell(tree.root_node(), source, None, 0, &mut symbols),
        Language::Php => walk_php_root(tree.root_node(), source, &mut symbols),
        Language::Kotlin => walk_kotlin(tree.root_node(), source, None, 0, &mut symbols),
        Language::Lua => walk_lua(tree.root_node(), source, None, 0, true, &mut symbols),
        Language::Hcl => walk_hcl(tree.root_node(), source, None, 0, &mut symbols),
        Language::R => walk_r(tree.root_node(), source, None, 0, &mut symbols),
        Language::Ruby => walk_ruby(tree.root_node(), source, None, 0, &mut symbols),
        Language::Swift => walk_swift(tree.root_node(), source, None, 0, &mut symbols),
        Language::Scala => walk_scala(tree.root_node(), source, None, 0, &mut symbols),
        Language::Dart => walk_dart(tree.root_node(), source, None, 0, &mut symbols),
        Language::Elixir => walk_elixir(tree.root_node(), source, None, 0, &mut symbols),
        Language::Julia => walk_julia(tree.root_node(), source, None, 0, &mut symbols),
        Language::Lean => walk_lean(tree.root_node(), source, &mut symbols),
        Language::Json | Language::Jsonc | Language::Yaml | Language::Toml | Language::Markdown => {
            unreachable!()
        }
    }
    (symbols.symbols, symbols.truncated)
}

#[derive(Default)]
struct SymbolCollector {
    symbols: Vec<Symbol>,
    text_bytes: usize,
    truncated: bool,
}

impl SymbolCollector {
    fn push(&mut self, symbol: Symbol) {
        if self.truncated {
            return;
        }
        let text_bytes = self.text_bytes.saturating_add(symbol.text_bytes());
        if self.symbols.len() >= MAX_CODE_SYMBOLS || text_bytes > MAX_CODE_SYMBOL_TEXT_BYTES {
            self.truncated = true;
            return;
        }
        self.text_bytes = text_bytes;
        self.symbols.push(symbol);
    }
}

fn push_symbol(
    node: Node<'_>,
    name_node: Node<'_>,
    source: &str,
    qualification: (Option<&str>, &str),
    kind: &'static str,
    depth: usize,
    output: &mut SymbolCollector,
) -> String {
    if cpp_path_name(name_node) {
        let name_path = cpp_name_path(name_node);
        let names = name_path
            .parts
            .into_iter()
            .map(|name| cpp_name_spelling(name, source))
            .collect::<Vec<_>>();
        let parent_path = qualification.0.filter(|_| !name_path.global).map_or_else(
            SymbolPath::default,
            |parent| {
                SymbolPath::parse_canonical(parent).expect("internal parent path must be canonical")
            },
        );
        // AST scope/name fields define hierarchy. `::` inside template arguments
        // is part of one segment, not another owner of this declaration.
        let path = parent_path.extend_names(names);
        let qualified = path.canonical();
        let legacy = path.legacy_code(qualification.1);
        return push_symbol_path(
            node,
            (path, qualified, legacy),
            declaration_name_position(name_node).map(|point| (point.row, point.column)),
            source,
            kind,
            depth,
            output,
        );
    }
    let name = source_slice(source, name_node.start_byte(), name_node.end_byte());
    push_symbol_name(
        node,
        (&name, declaration_name_position(name_node)),
        source,
        qualification,
        kind,
        depth,
        output,
    )
}

// Keep qualified display names, but query the declared member rather than its owner.
fn declaration_name_position(node: Node<'_>) -> Option<Point> {
    if matches!(node.kind(), "operator_name" | "operator_cast") {
        return Some(node.start_position());
    }
    if node.kind() == "destructor_name" {
        return named_child_with_kind(&node, &["identifier", "type_identifier"])
            .map(|name| name.start_position());
    }
    for field in ["name", "field", "method"] {
        if let Some(name) = node.child_by_field_name(field) {
            return declaration_name_position(name);
        }
    }
    if node.kind() == "field_expression" {
        // Julia gives the receiver a `value` field; the member is the final child.
        let member = node.named_child(node.named_child_count().checked_sub(1)?.try_into().ok()?)?;
        return (member.kind() == "identifier").then(|| member.start_position());
    }
    match node.named_child_count() {
        0 => Some(node.start_position()),
        1 => declaration_name_position(node.named_child(0)?),
        _ => None,
    }
}

fn push_symbol_name(
    node: Node<'_>,
    name: (&str, Option<Point>),
    source: &str,
    qualification: (Option<&str>, &str),
    kind: &'static str,
    depth: usize,
    output: &mut SymbolCollector,
) -> String {
    let name_position = name.1.map(|point| (point.row, point.column));
    let name = one_line(name.0);
    let (path, qualified, legacy_qualified_name) =
        qualified_names(qualification.0, &name, qualification.1, output);
    push_symbol_path(
        node,
        (path, qualified, legacy_qualified_name),
        name_position,
        source,
        kind,
        depth,
        output,
    )
}

fn push_symbol_path(
    node: Node<'_>,
    names: (SymbolPath, String, String),
    name_position: Option<(usize, usize)>,
    source: &str,
    kind: &'static str,
    depth: usize,
    output: &mut SymbolCollector,
) -> String {
    let (path, qualified, legacy_qualified_name) = names;
    output.push(Symbol {
        kind,
        path,
        qualified_name: qualified.clone(),
        legacy_qualified_name,
        signature: signature(node, source, node.start_byte()),
        name_position,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_row: node.start_position().row,
        start_column: node.start_position().column,
        end_row: node.end_position().row,
        end_column: node.end_position().column,
        depth,
    });
    qualified
}

fn push_literal_symbol(
    node: Node<'_>,
    name: (Node<'_>, &str),
    source: &str,
    parent: Option<&str>,
    kind: &'static str,
    depth: usize,
    output: &mut SymbolCollector,
) -> String {
    let (name_node, name) = name;
    let path = qualified_path(parent, name, "");
    let qualified = path.canonical();
    let legacy = path.legacy_code(".");
    let position = declaration_name_position(name_node).map(|point| (point.row, point.column));
    push_symbol_path(
        node,
        (path, qualified, legacy),
        position,
        source,
        kind,
        depth,
        output,
    )
}

fn add_pattern_bindings(
    declaration: Node<'_>,
    target: Node<'_>,
    source: &str,
    parent: Option<&str>,
    kind: &'static str,
    depth: usize,
    output: &mut SymbolCollector,
) {
    match target.kind() {
        "identifier" | "simple_identifier" | "shorthand_property_identifier_pattern" => {
            if target.kind() != "simple_identifier"
                || source_slice(source, target.start_byte(), target.end_byte()) != "_"
            {
                push_symbol(
                    declaration,
                    target,
                    source,
                    (parent, "."),
                    kind,
                    depth,
                    output,
                );
            }
        }
        "pair_pattern" => {
            if let Some(value) = target.child_by_field_name("value") {
                add_pattern_bindings(declaration, value, source, parent, kind, depth, output);
            }
        }
        "typed_pattern" => {
            if let Some(pattern) = target.child_by_field_name("pattern") {
                add_pattern_bindings(declaration, pattern, source, parent, kind, depth, output);
            }
        }
        "assignment_pattern" | "object_assignment_pattern" => {
            if let Some(left) = target.child_by_field_name("left") {
                add_pattern_bindings(declaration, left, source, parent, kind, depth, output);
            }
        }
        "pattern"
        | "tuple_pattern"
        | "list_pattern"
        | "pattern_list"
        | "array_pattern"
        | "object_pattern"
        | "rest_pattern"
        | "list_splat_pattern"
        | "multi_variable_declaration"
        | "variable_declaration"
        | "identifiers"
        | "tuple"
        | "list" => {
            walk_named_children(target, |child| {
                add_pattern_bindings(declaration, child, source, parent, kind, depth, output);
            });
        }
        _ => {}
    }
}

fn walk_java(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if node.kind() == "field_declaration" && parent.is_some() {
        let mut cursor = node.walk();
        let declarators = node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "variable_declarator")
            .filter_map(|child| child.child_by_field_name("name"))
            .collect::<Vec<_>>();
        for name_node in declarators {
            push_symbol(
                node,
                name_node,
                source,
                (parent, "."),
                "field",
                depth,
                output,
            );
        }
        return;
    }
    let kind = match node.kind() {
        "class_declaration" => Some("class"),
        "interface_declaration" => Some("interface"),
        "enum_declaration" => Some("enum"),
        "record_declaration" => Some("record"),
        "annotation_type_declaration" => Some("annotation"),
        "constructor_declaration" => Some("constructor"),
        "method_declaration" => Some("method"),
        "enum_constant" if parent.is_some() => Some("variant"),
        _ => None,
    };
    if let Some(kind) = kind {
        let name_node = node.child_by_field_name("name");
        if let Some(name_node) = name_node {
            let qualified =
                push_symbol(node, name_node, source, (parent, "."), kind, depth, output);
            if matches!(
                node.kind(),
                "class_declaration"
                    | "interface_declaration"
                    | "enum_declaration"
                    | "record_declaration"
                    | "annotation_type_declaration"
            ) && let Some(body) = node.child_by_field_name("body")
            {
                walk_named_children(body, |child| {
                    walk_java(child, source, Some(&qualified), depth + 1, output)
                });
            }
            return;
        }
    }
    walk_named_children(node, |child| {
        walk_java(child, source, parent, depth, output)
    });
}

// Qualified C++ names are recursive scope/name nodes, not raw strings split on `::`.
struct CppNamePath<'tree> {
    parts: Vec<Node<'tree>>,
    global: bool,
}

fn cpp_path_name(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "qualified_identifier"
            | "nested_namespace_specifier"
            | "destructor_name"
            | "template_type"
            | "template_function"
    )
}

fn cpp_name_path(node: Node<'_>) -> CppNamePath<'_> {
    fn collect<'tree>(node: Node<'tree>, path: &mut CppNamePath<'tree>) {
        if node.kind() == "nested_namespace_specifier" {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor).filter(|child| {
                matches!(
                    child.kind(),
                    "namespace_identifier" | "nested_namespace_specifier"
                )
            }) {
                collect(child, path);
            }
        } else if node.kind() == "qualified_identifier" {
            if let Some(scope) = node.child_by_field_name("scope") {
                collect(scope, path);
            } else if node.child(0).is_some_and(|token| token.kind() == "::") {
                // The grammar represents a leading global `::` without a scope field.
                path.global = true;
            }
            if let Some(name) = node.child_by_field_name("name") {
                collect(name, path);
            }
        } else {
            // Template arguments (including their own `::` or literals) stay inside
            // one atom; only syntactic qualification adds declaration ancestry.
            path.parts.push(node);
        }
    }
    let mut path = CppNamePath {
        parts: Vec::new(),
        global: false,
    };
    collect(node, &mut path);
    path
}

fn cpp_name_spelling(node: Node<'_>, source: &str) -> String {
    fn tokens<'tree>(node: Node<'tree>, output: &mut Vec<Node<'tree>>) {
        if node.kind() == "comment" {
            return;
        }
        // Literal contents and escapes are semantic spelling, not name trivia.
        if node.child_count() == 0
            || matches!(
                node.kind(),
                "string_literal" | "raw_string_literal" | "char_literal"
            )
        {
            output.push(node);
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            tokens(child, output);
        }
    }
    let mut leaves = Vec::new();
    tokens(node, &mut leaves);
    let compact = matches!(node.kind(), "destructor_name" | "operator_name");
    let mut spelling = String::new();
    let mut previous_end = None;
    for token in leaves {
        let text = source_slice(source, token.start_byte(), token.end_byte());
        let word_boundary = spelling.ends_with(|c: char| c.is_alphanumeric() || c == '_')
            && text.starts_with(|c: char| c.is_alphanumeric() || c == '_');
        if previous_end.is_some_and(|end| end < token.start_byte()) && (!compact || word_boundary) {
            spelling.push(' ');
        }
        spelling.push_str(&text);
        previous_end = Some(token.end_byte());
    }
    spelling
}

fn c_family_function_kind(node: Node<'_>, name: Node<'_>, source: &str, cpp: bool) -> &'static str {
    if !cpp {
        return "function";
    }
    let names = cpp_name_path(name).parts;
    let class = enclosing_class_like(node);
    if names.len() < 2 && class.is_none() {
        return "function";
    }
    let leaf = names
        .last()
        .map(|name| cpp_name_spelling(name.child_by_field_name("name").unwrap_or(*name), source));
    let owner_node = names.iter().rev().nth(1).copied().or_else(|| {
        class
            .and_then(|class| class.child_by_field_name("name"))
            .and_then(|name| cpp_name_path(name).parts.last().copied())
    });
    // A constructor names its class, not the class's template arguments.
    let owner = owner_node
        .map(|owner| cpp_name_spelling(owner.child_by_field_name("name").unwrap_or(owner), source));
    if owner.is_some() && owner == leaf {
        "constructor"
    } else {
        "method"
    }
}

fn walk_c_family(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    cpp: bool,
    output: &mut SymbolCollector,
) {
    let container_kind = match node.kind() {
        "namespace_definition" if cpp => Some("namespace"),
        "class_specifier" if cpp => Some("class"),
        "struct_specifier" => Some("struct"),
        "union_specifier" => Some("union"),
        "enum_specifier" => Some("enum"),
        _ => None,
    };
    if let Some(kind) = container_kind
        && let Some(name_node) = node.child_by_field_name("name")
    {
        let qualified = push_symbol(node, name_node, source, (parent, "::"), kind, depth, output);
        if let Some(body) = node.child_by_field_name("body") {
            walk_named_children(body, |child| {
                walk_c_family(child, source, Some(&qualified), depth + 1, cpp, output)
            });
        }
        return;
    }
    if node.kind() == "enumerator"
        && parent.is_some()
        && let Some(name_node) = node.child_by_field_name("name")
    {
        push_symbol(
            node,
            name_node,
            source,
            (parent, "::"),
            "variant",
            depth,
            output,
        );
        return;
    }
    if node.kind() == "function_definition"
        && let Some(declarator) = node.child_by_field_name("declarator")
        && let Some(name_node) = declarator_name(declarator)
    {
        let kind = c_family_function_kind(node, name_node, source, cpp);
        push_symbol(node, name_node, source, (parent, "::"), kind, depth, output);
        return;
    }
    if matches!(node.kind(), "declaration" | "field_declaration") {
        let mut cursor = node.walk();
        let mut declared = false;
        for declarator in node
            .children_by_field_name("declarator", &mut cursor)
            .filter(Node::is_named)
        {
            let Some(name_node) = declarator_name(declarator) else {
                continue;
            };
            let function = declarator.kind() == "function_declarator"
                || descendant_with_kind(declarator, "function_declarator").is_some();
            let kind = if function {
                c_family_function_kind(node, name_node, source, cpp)
            } else if parent.is_some() {
                "field"
            } else {
                continue;
            };
            push_symbol(node, name_node, source, (parent, "::"), kind, depth, output);
            declared = true;
        }
        if declared {
            return;
        }
    }
    if cpp
        && node.kind() == "function_declarator"
        && let Some(name_node) = declarator_name(node)
    {
        let kind = c_family_function_kind(node, name_node, source, cpp);
        let item = node
            .parent()
            .filter(|candidate| candidate.kind() == "template_declaration")
            .unwrap_or(node);
        push_symbol(item, name_node, source, (parent, "::"), kind, depth, output);
        return;
    }
    walk_named_children(node, |child| {
        walk_c_family(child, source, parent, depth, cpp, output)
    });
}

fn walk_bash(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if node.kind() == "function_definition"
        && let Some(name_node) = node.child_by_field_name("name")
    {
        let qualified = push_symbol(
            node,
            name_node,
            source,
            (parent, "."),
            "function",
            depth,
            output,
        );
        if let Some(body) = node.child_by_field_name("body") {
            walk_named_children(body, |child| {
                walk_bash(child, source, Some(&qualified), depth + 1, output)
            });
        }
        return;
    }
    walk_named_children(node, |child| {
        walk_bash(child, source, parent, depth, output)
    });
}

fn walk_go(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if node.kind() == "type_spec"
        && let Some(name_node) = node.child_by_field_name("name")
    {
        let type_node = node.child_by_field_name("type");
        let kind = match type_node.map(|child| child.kind()) {
            Some("struct_type") => "struct",
            Some("interface_type") => "interface",
            _ => "type",
        };
        let qualified = push_symbol(node, name_node, source, (parent, "."), kind, depth, output);
        if let Some(body) = type_node {
            walk_named_children(body, |child| {
                walk_go(child, source, Some(&qualified), depth + 1, output)
            });
        }
        return;
    }
    if matches!(node.kind(), "function_declaration" | "method_declaration")
        && let Some(name_node) = node.child_by_field_name("name")
    {
        let owner = if node.kind() == "method_declaration" {
            node.child_by_field_name("receiver")
                .and_then(|receiver| {
                    descendant_with_kind(receiver, "type_identifier")
                        .or_else(|| descendant_type_name(receiver))
                })
                .map(|receiver| {
                    one_line(&source_slice(
                        source,
                        receiver.start_byte(),
                        receiver.end_byte(),
                    ))
                })
        } else {
            None
        };
        push_symbol(
            node,
            name_node,
            source,
            (owner.as_deref().or(parent), "."),
            if node.kind() == "method_declaration" {
                "method"
            } else {
                "function"
            },
            depth,
            output,
        );
        return;
    }
    if node.kind() == "method_elem"
        && parent.is_some()
        && let Some(name_node) = node.child_by_field_name("name")
    {
        push_symbol(
            node,
            name_node,
            source,
            (parent, "."),
            "method",
            depth,
            output,
        );
        return;
    }
    if matches!(node.kind(), "const_spec" | "var_spec") && parent.is_none() {
        let mut cursor = node.walk();
        for name_node in node
            .children_by_field_name("name", &mut cursor)
            .filter(Node::is_named)
        {
            if source_slice(source, name_node.start_byte(), name_node.end_byte()) == "_" {
                continue;
            }
            push_symbol(
                node,
                name_node,
                source,
                (None, "."),
                "binding",
                depth,
                output,
            );
        }
        return;
    }
    if node.kind() == "field_declaration" && parent.is_some() {
        let mut cursor = node.walk();
        for name_node in node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "field_identifier")
        {
            push_symbol(
                node,
                name_node,
                source,
                (parent, "."),
                "field",
                depth,
                output,
            );
        }
        return;
    }
    walk_named_children(node, |child| walk_go(child, source, parent, depth, output));
}

fn walk_ecmascript(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    typescript: bool,
    output: &mut SymbolCollector,
) {
    let container_kind = match node.kind() {
        "class_declaration" | "abstract_class_declaration" => Some("class"),
        "interface_declaration" if typescript => Some("interface"),
        "enum_declaration" if typescript => Some("enum"),
        "internal_module" if typescript => Some("namespace"),
        _ => None,
    };
    if let Some(kind) = container_kind
        && let Some(name_node) = node.child_by_field_name("name")
    {
        let qualified =
            push_ecmascript_symbol(node, name_node, source, parent, kind, depth, output);
        if let Some(body) = node.child_by_field_name("body") {
            walk_named_children(body, |child| {
                walk_ecmascript(
                    child,
                    source,
                    Some(&qualified),
                    depth + 1,
                    typescript,
                    output,
                )
            });
        }
        return;
    }
    let declaration_kind = match node.kind() {
        "function_declaration" | "generator_function_declaration" => Some("function"),
        "method_definition" | "method_signature" => Some("method"),
        "type_alias_declaration" if typescript => Some("type"),
        _ => None,
    };
    if let Some(kind) = declaration_kind
        && let Some(name_node) = node.child_by_field_name("name")
    {
        let kind = if kind == "method"
            && source_slice(source, name_node.start_byte(), name_node.end_byte()) == "constructor"
        {
            "constructor"
        } else {
            kind
        };
        let qualified =
            push_ecmascript_symbol(node, name_node, source, parent, kind, depth, output);
        if matches!(
            node.kind(),
            "function_declaration"
                | "generator_function_declaration"
                | "method_definition"
                | "method_signature"
        ) && let Some(body) = node.child_by_field_name("body")
        {
            walk_named_children(body, |child| {
                walk_ecmascript(
                    child,
                    source,
                    Some(&qualified),
                    depth + 1,
                    typescript,
                    output,
                )
            });
        }
        return;
    }
    if matches!(node.kind(), "enum_assignment" | "enum_member")
        && parent.is_some()
        && let Some(name_node) = node.child_by_field_name("name").or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor).next()
        })
    {
        push_ecmascript_symbol(node, name_node, source, parent, "variant", depth, output);
        return;
    }
    if matches!(
        node.kind(),
        "public_field_definition" | "field_definition" | "property_signature"
    ) && parent.is_some()
        && let Some(name_node) = node.child_by_field_name("name")
    {
        push_ecmascript_symbol(node, name_node, source, parent, "field", depth, output);
        return;
    }
    if node.kind() == "variable_declarator" {
        let function_value = node.child_by_field_name("value").is_some_and(|value| {
            matches!(
                value.kind(),
                "arrow_function" | "function_expression" | "generator_function"
            )
        });
        if (is_program_level(node) || function_value)
            && let Some(name_node) = node.child_by_field_name("name")
        {
            if name_node.kind() != "identifier" {
                add_pattern_bindings(node, name_node, source, parent, "binding", depth, output);
                return;
            }
            let kind = if function_value {
                "function"
            } else {
                "binding"
            };
            let qualified =
                push_ecmascript_symbol(node, name_node, source, parent, kind, depth, output);
            if function_value
                && let Some(value) = node.child_by_field_name("value")
                && let Some(body) = value.child_by_field_name("body")
            {
                walk_named_children(body, |child| {
                    walk_ecmascript(
                        child,
                        source,
                        Some(&qualified),
                        depth + 1,
                        typescript,
                        output,
                    )
                });
            }
            return;
        }
    }
    walk_named_children(node, |child| {
        walk_ecmascript(child, source, parent, depth, typescript, output)
    });
}

fn push_ecmascript_symbol(
    node: Node<'_>,
    name_node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    kind: &'static str,
    depth: usize,
    output: &mut SymbolCollector,
) -> String {
    if name_node.kind() == "string" {
        let raw = source_slice(source, name_node.start_byte(), name_node.end_byte());
        let Some(name) = document::decode_quoted_scalar(&raw, Language::JavaScript) else {
            output.truncated = true;
            return String::new();
        };
        let kind = if kind == "method" && name == "constructor" {
            "constructor"
        } else {
            kind
        };
        return push_literal_symbol(
            node,
            (name_node, &name),
            source,
            parent,
            kind,
            depth,
            output,
        );
    }
    push_symbol(node, name_node, source, (parent, "."), kind, depth, output)
}

fn walk_csharp_root(node: Node<'_>, source: &str, output: &mut SymbolCollector) {
    let mut namespace = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "file_scoped_namespace_declaration"
            && let Some(name_node) = child.child_by_field_name("name")
        {
            namespace = Some(push_symbol(
                child,
                name_node,
                source,
                (None, "."),
                "namespace",
                0,
                output,
            ));
        } else {
            walk_csharp(
                child,
                source,
                namespace.as_deref(),
                usize::from(namespace.is_some()),
                output,
            );
        }
    }
}

fn walk_csharp(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    let container_kind = match node.kind() {
        "namespace_declaration" => Some("namespace"),
        "class_declaration" => Some("class"),
        "interface_declaration" => Some("interface"),
        "struct_declaration" => Some("struct"),
        "record_declaration" => Some("record"),
        "enum_declaration" => Some("enum"),
        _ => None,
    };
    if let Some(kind) = container_kind
        && let Some(name_node) = node.child_by_field_name("name")
    {
        let qualified = push_symbol(node, name_node, source, (parent, "."), kind, depth, output);
        if let Some(body) = node.child_by_field_name("body") {
            walk_named_children(body, |child| {
                walk_csharp(child, source, Some(&qualified), depth + 1, output)
            });
        } else {
            walk_named_children(node, |child| {
                if child != name_node {
                    walk_csharp(child, source, Some(&qualified), depth + 1, output)
                }
            });
        }
        return;
    }
    let member_kind = match node.kind() {
        "method_declaration" => Some("method"),
        "constructor_declaration" => Some("constructor"),
        "property_declaration" => Some("property"),
        "enum_member_declaration" => Some("variant"),
        _ => None,
    };
    if let Some(kind) = member_kind
        && let Some(name_node) = node.child_by_field_name("name")
    {
        push_symbol(node, name_node, source, (parent, "."), kind, depth, output);
        return;
    }
    if node.kind() == "operator_declaration"
        && let Some(operator) = node.child_by_field_name("operator")
    {
        let name = format!(
            "operator{}",
            source_slice(source, operator.start_byte(), operator.end_byte())
        );
        push_symbol_name(
            node,
            (&name, Some(operator.start_position())),
            source,
            (parent, "."),
            "operator",
            depth,
            output,
        );
        return;
    }
    if node.kind() == "conversion_operator_declaration"
        && let Some(target_type) = node.child_by_field_name("type")
    {
        let name = format!(
            "operator:{}",
            one_line(&source_slice(
                source,
                target_type.start_byte(),
                target_type.end_byte()
            ))
        );
        push_symbol_name(
            node,
            (&name, None),
            source,
            (parent, "."),
            "operator",
            depth,
            output,
        );
        return;
    }
    if node.kind() == "field_declaration" && parent.is_some() {
        let mut cursor = node.walk();
        for declarator in node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "variable_declaration")
            .flat_map(|declaration| {
                let mut nested = declaration.walk();
                declaration
                    .named_children(&mut nested)
                    .filter(|child| child.kind() == "variable_declarator")
                    .collect::<Vec<_>>()
            })
        {
            if let Some(name_node) = declarator.child_by_field_name("name") {
                push_symbol(
                    node,
                    name_node,
                    source,
                    (parent, "."),
                    "field",
                    depth,
                    output,
                );
            }
        }
        return;
    }
    walk_named_children(node, |child| {
        walk_csharp(child, source, parent, depth, output)
    });
}

fn walk_powershell(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    let declaration = match node.kind() {
        "class_statement" => Some(("simple_name", "class", ".")),
        "enum_statement" => Some(("simple_name", "enum", ".")),
        "function_statement" => Some(("function_name", "function", "::")),
        "class_method_definition" => Some(("simple_name", "method", "::")),
        "class_property_definition" => Some(("variable", "field", ".")),
        "enum_member" => Some(("simple_name", "variant", ".")),
        _ => None,
    };
    if let Some((name_kind, mut kind, separator)) = declaration
        && let Some(name) = named_child_with_kind(&node, &[name_kind])
    {
        if node.kind() == "function_statement" && parent.is_some() {
            kind = "method";
        }
        let qualified = push_symbol(node, name, source, (parent, separator), kind, depth, output);
        walk_named_children(node, |child| {
            if child != name {
                walk_powershell(child, source, Some(&qualified), depth + 1, output);
            }
        });
        return;
    }
    walk_named_children(node, |child| {
        walk_powershell(child, source, parent, depth, output)
    });
}

fn walk_php_root(root: Node<'_>, source: &str, output: &mut SymbolCollector) {
    let mut namespace = None;
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "namespace_definition" {
            walk_php(child, source, namespace.as_deref(), 0, output);
            continue;
        }
        let Some(name) = child.child_by_field_name("name") else {
            if let Some(body) = child.child_by_field_name("body") {
                walk_php(body, source, None, 0, output);
            }
            namespace = None;
            continue;
        };
        let namespace_name = push_symbol(child, name, source, (None, "\\"), "namespace", 0, output);
        if let Some(body) = child.child_by_field_name("body") {
            walk_php(body, source, Some(&namespace_name), 1, output);
        } else {
            namespace = Some(namespace_name);
        }
    }
}

fn walk_php(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    let declaration = match node.kind() {
        "class_declaration" => Some(("class", "\\")),
        "interface_declaration" => Some(("interface", "\\")),
        "trait_declaration" => Some(("trait", "\\")),
        "enum_declaration" => Some(("enum", "\\")),
        "function_definition" => Some(("function", "\\")),
        "method_declaration" => Some(("method", "::")),
        "enum_case" => Some(("variant", "::")),
        _ => None,
    };
    if let Some((kind, separator)) = declaration
        && let Some(name) = node.child_by_field_name("name")
    {
        let qualified = push_symbol(node, name, source, (parent, separator), kind, depth, output);
        walk_named_children(node, |child| {
            if child != name {
                walk_php(child, source, Some(&qualified), depth + 1, output);
            }
        });
        return;
    }
    if node.kind() == "property_declaration" {
        let mut cursor = node.walk();
        for element in node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "property_element")
        {
            if let Some(name) = element.child_by_field_name("name") {
                push_symbol(node, name, source, (parent, "::"), "field", depth, output);
            }
        }
        return;
    }
    walk_named_children(node, |child| walk_php(child, source, parent, depth, output));
}

fn walk_kotlin(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if node.kind() == "property_declaration"
        && let Some(target) = named_child_with_kind(&node, &["multi_variable_declaration"])
    {
        add_pattern_bindings(
            node,
            target,
            source,
            parent,
            if parent.is_some() { "field" } else { "binding" },
            depth,
            output,
        );
        return;
    }
    let (name, kind, separator) = match node.kind() {
        "class_declaration" => {
            let head = signature(node, source, node.start_byte());
            let kind = if head.contains("interface ") {
                "interface"
            } else if head.contains("enum class ") {
                "enum"
            } else {
                "class"
            };
            (
                named_child_with_kind(&node, &["type_identifier"]),
                kind,
                ".",
            )
        }
        "object_declaration" | "companion_object" => (
            named_child_with_kind(&node, &["type_identifier"]),
            "object",
            ".",
        ),
        "function_declaration" => (
            named_child_with_kind(&node, &["simple_identifier"]),
            if parent.is_some() {
                "method"
            } else {
                "function"
            },
            ".",
        ),
        "property_declaration" => (
            named_child_with_kind(&node, &["variable_declaration"])
                .and_then(|child| descendant_with_kind(child, "simple_identifier")),
            if parent.is_some() { "field" } else { "binding" },
            ".",
        ),
        "type_alias" => (
            named_child_with_kind(&node, &["type_identifier"]),
            "type",
            ".",
        ),
        "enum_entry" => (
            named_child_with_kind(&node, &["simple_identifier"]),
            "variant",
            ".",
        ),
        _ => (None, "", "."),
    };
    if let Some(name) = name {
        let qualified = push_symbol(node, name, source, (parent, separator), kind, depth, output);
        walk_named_children(node, |child| {
            if child != name {
                walk_kotlin(child, source, Some(&qualified), depth + 1, output);
            }
        });
        return;
    }
    walk_named_children(node, |child| {
        walk_kotlin(child, source, parent, depth, output)
    });
}

fn walk_lua(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    top_level: bool,
    output: &mut SymbolCollector,
) {
    if node.kind() == "function_declaration"
        && let Some(name) = node.child_by_field_name("name")
    {
        let qualified = push_symbol(
            node,
            name,
            source,
            (parent, "."),
            if parent.is_some() {
                "method"
            } else {
                "function"
            },
            depth,
            output,
        );
        if let Some(body) = node.child_by_field_name("body") {
            walk_lua(body, source, Some(&qualified), depth + 1, false, output);
        }
        return;
    }
    if matches!(node.kind(), "variable_declaration" | "assignment_statement")
        && let Some(assignment) = if node.kind() == "assignment_statement" {
            Some(node)
        } else {
            named_child_with_kind(&node, &["assignment_statement"])
        }
        && let Some(variables) = named_child_with_kind(&assignment, &["variable_list"])
        && let Some(values) = named_child_with_kind(&assignment, &["expression_list"])
        && let Some(value) = values.named_child(0)
        && value.kind() == "function_definition"
        && let Some(name) = variables.child_by_field_name("name")
    {
        let qualified = push_symbol(
            node,
            name,
            source,
            (parent, "."),
            if parent.is_some() {
                "method"
            } else {
                "function"
            },
            depth,
            output,
        );
        if let Some(body) = value.child_by_field_name("body") {
            walk_lua(body, source, Some(&qualified), depth + 1, false, output);
        }
        return;
    }
    if top_level && node.kind() == "variable_declaration" {
        if let Some(list) = named_child_with_kind(&node, &["variable_list"]) {
            let mut cursor = list.walk();
            for name in list.children_by_field_name("name", &mut cursor) {
                push_symbol(node, name, source, (parent, "."), "binding", depth, output);
            }
        }
        return;
    }
    walk_named_children(node, |child| {
        walk_lua(child, source, parent, depth, top_level, output)
    });
}

fn walk_hcl(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if node.kind() == "block" {
        let mut cursor = node.walk();
        let segments = node
            .named_children(&mut cursor)
            .filter(|child| matches!(child.kind(), "identifier" | "string_lit"))
            .map(|child| {
                let raw = source_slice(source, child.start_byte(), child.end_byte());
                if child.kind() == "string_lit" {
                    document::decode_quoted_scalar(&raw, Language::Toml)
                } else {
                    Some(raw.into_owned())
                }
            })
            .collect::<Option<Vec<_>>>();
        let Some(segments) = segments else {
            output.truncated = true;
            return;
        };
        if !segments.is_empty() {
            let path = qualified_path(parent, "", "::").extend_names(segments);
            let names = (path.clone(), path.canonical(), path.legacy_code("."));
            let qualified = push_symbol_path(node, names, None, source, "block", depth, output);
            if let Some(body) = named_child_with_kind(&node, &["body"]) {
                walk_hcl(body, source, Some(&qualified), depth + 1, output);
            }
            return;
        }
    }
    if node.kind() == "attribute"
        && let Some(name) = named_child_with_kind(&node, &["identifier"])
    {
        push_symbol(
            node,
            name,
            source,
            (parent, "."),
            "attribute",
            depth,
            output,
        );
        return;
    }
    walk_named_children(node, |child| walk_hcl(child, source, parent, depth, output));
}

fn walk_r(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if node.kind() == "binary_operator"
        && let Some(operator) = node.child_by_field_name("operator")
    {
        let operator_text = source_slice(source, operator.start_byte(), operator.end_byte());
        let (name, function) = if matches!(operator_text.as_ref(), "<-" | "<<-" | "=") {
            (
                node.child_by_field_name("lhs"),
                node.child_by_field_name("rhs"),
            )
        } else if matches!(operator_text.as_ref(), "->" | "->>") {
            (
                node.child_by_field_name("rhs"),
                node.child_by_field_name("lhs"),
            )
        } else {
            (None, None)
        };
        let function = function.map(unwrap_r_parentheses);
        if let (Some(name), Some(function)) = (name, function)
            && function.kind() == "function_definition"
            && matches!(name.kind(), "identifier" | "string")
        {
            let raw = source_slice(source, name.start_byte(), name.end_byte());
            let value = if name.kind() == "string" || raw.starts_with('`') {
                document::decode_quoted_scalar(&raw, Language::R)
            } else {
                Some(raw.into_owned())
            };
            let Some(value) = value else {
                output.truncated = true;
                return;
            };
            let qualified = push_literal_symbol(
                node,
                (name, &value),
                source,
                parent,
                "function",
                depth,
                output,
            );
            if let Some(body) = function.child_by_field_name("body") {
                walk_r(body, source, Some(&qualified), depth + 1, output);
            }
            return;
        }
    }
    walk_named_children(node, |child| walk_r(child, source, parent, depth, output));
}

fn unwrap_r_parentheses(mut node: Node<'_>) -> Node<'_> {
    while node.kind() == "parenthesized_expression" {
        let Some(body) = node.child_by_field_name("body") else {
            break;
        };
        node = body;
    }
    node
}

fn walk_ruby(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if matches!(node.kind(), "module" | "class")
        && let Some(name) = node.child_by_field_name("name")
    {
        let qualified = push_symbol(
            node,
            name,
            source,
            (parent, "::"),
            node.kind(),
            depth,
            output,
        );
        if let Some(body) = node.child_by_field_name("body") {
            walk_ruby(body, source, Some(&qualified), depth + 1, output);
        }
        return;
    }
    if matches!(node.kind(), "method" | "singleton_method")
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(
            node,
            name,
            source,
            (parent, "."),
            if parent.is_some() {
                "method"
            } else {
                "function"
            },
            depth,
            output,
        );
        return;
    }
    if parent.is_none()
        && node.kind() == "assignment"
        && let Some(name) = node.child_by_field_name("left")
        && name.kind() == "constant"
    {
        push_symbol(node, name, source, (None, "::"), "constant", depth, output);
        return;
    }
    walk_named_children(node, |child| {
        walk_ruby(child, source, parent, depth, output)
    });
}

fn walk_swift(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    let declaration = match node.kind() {
        "protocol_declaration" => Some(("protocol", node.child_by_field_name("name"))),
        "class_declaration" => {
            let head = signature(node, source, node.start_byte());
            let kind = if head.trim_start().starts_with("extension ") {
                "extension"
            } else if head.contains(" enum ") || head.trim_start().starts_with("enum ") {
                "enum"
            } else if head.contains(" struct ") || head.trim_start().starts_with("struct ") {
                "struct"
            } else if head.contains(" actor ") || head.trim_start().starts_with("actor ") {
                "actor"
            } else {
                "class"
            };
            Some((kind, node.child_by_field_name("name")))
        }
        "typealias_declaration" => Some(("type", node.child_by_field_name("name"))),
        _ => None,
    };
    if let Some((kind, Some(name))) = declaration {
        let qualified = push_symbol(node, name, source, (parent, "."), kind, depth, output);
        walk_named_children(node, |child| {
            if child != name {
                walk_swift(child, source, Some(&qualified), depth + 1, output);
            }
        });
        return;
    }
    if matches!(
        node.kind(),
        "function_declaration" | "protocol_function_declaration"
    ) && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(
            node,
            name,
            source,
            (parent, "."),
            if parent.is_some() {
                "method"
            } else {
                "function"
            },
            depth,
            output,
        );
        return;
    }
    if matches!(node.kind(), "init_declaration" | "deinit_declaration") {
        push_symbol_name(
            node,
            (
                if node.kind() == "init_declaration" {
                    "init"
                } else {
                    "deinit"
                },
                None,
            ),
            source,
            (parent, "."),
            "method",
            depth,
            output,
        );
        return;
    }
    if matches!(
        node.kind(),
        "property_declaration" | "protocol_property_declaration"
    ) {
        let mut cursor = node.walk();
        for target in node
            .children_by_field_name("name", &mut cursor)
            .filter(Node::is_named)
        {
            add_pattern_bindings(
                node,
                target,
                source,
                parent,
                if parent.is_some() {
                    "property"
                } else {
                    "binding"
                },
                depth,
                output,
            );
        }
        return;
    }
    if node.kind() == "enum_entry"
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(node, name, source, (parent, "."), "variant", depth, output);
        return;
    }
    walk_named_children(node, |child| {
        walk_swift(child, source, parent, depth, output)
    });
}

fn walk_scala(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    let kind = match node.kind() {
        "class_definition" => "class",
        "trait_definition" => "trait",
        "object_definition" => "object",
        "enum_definition" => "enum",
        _ => "",
    };
    if !kind.is_empty()
        && let Some(name) = node.child_by_field_name("name")
    {
        let qualified = push_symbol(node, name, source, (parent, "."), kind, depth, output);
        walk_named_children(node, |child| {
            if child != name {
                walk_scala(child, source, Some(&qualified), depth + 1, output);
            }
        });
        return;
    }
    if matches!(node.kind(), "function_definition" | "function_declaration")
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(
            node,
            name,
            source,
            (parent, "."),
            if parent.is_some() {
                "method"
            } else {
                "function"
            },
            depth,
            output,
        );
        return;
    }
    if node.kind() == "type_definition"
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(node, name, source, (parent, "."), "type", depth, output);
        return;
    }
    if matches!(node.kind(), "val_definition" | "var_definition") {
        if let Some(target) = node
            .child_by_field_name("pattern")
            .or_else(|| named_child_with_kind(&node, &["identifier"]))
        {
            add_pattern_bindings(
                node,
                target,
                source,
                parent,
                if parent.is_some() { "field" } else { "binding" },
                depth,
                output,
            );
        }
        return;
    }
    if node.kind() == "class_parameter"
        && parent.is_some()
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(node, name, source, (parent, "."), "field", depth, output);
        return;
    }
    if matches!(node.kind(), "simple_enum_case" | "class_enum_case")
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(node, name, source, (parent, "."), "variant", depth, output);
        return;
    }
    walk_named_children(node, |child| {
        walk_scala(child, source, parent, depth, output)
    });
}

fn walk_dart(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    let kind = match node.kind() {
        "class_declaration" => "class",
        "enum_declaration" => "enum",
        "mixin_declaration" => "mixin",
        "extension_declaration" => "extension",
        "extension_type_declaration" => "extension-type",
        _ => "",
    };
    if !kind.is_empty()
        && let Some(name) = node.child_by_field_name("name")
    {
        let qualified = push_symbol(node, name, source, (parent, "."), kind, depth, output);
        walk_named_children(node, |child| {
            if child != name {
                walk_dart(child, source, Some(&qualified), depth + 1, output);
            }
        });
        return;
    }
    if matches!(
        node.kind(),
        "function_declaration" | "getter_declaration" | "setter_declaration"
    ) && let Some(name) = descendant_with_kind(node, "identifier")
    {
        push_symbol(node, name, source, (parent, "."), "function", depth, output);
        return;
    }
    if node.kind() == "method_declaration"
        && let Some(signature_node) = node
            .child_by_field_name("signature")
            .or_else(|| named_child_with_kind(&node, &["method_signature"]))
        && let Some(name) = descendant_with_kind(signature_node, "identifier")
    {
        push_symbol(node, name, source, (parent, "."), "method", depth, output);
        return;
    }
    if matches!(node.kind(), "getter_signature" | "setter_signature")
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(node, name, source, (parent, "."), "method", depth, output);
        return;
    }
    if node.kind() == "constructor_signature"
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(
            node,
            name,
            source,
            (parent, "."),
            "constructor",
            depth,
            output,
        );
        return;
    }
    if node.kind() == "initialized_identifier"
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(
            node,
            name,
            source,
            (parent, "."),
            if parent.is_some() { "field" } else { "binding" },
            depth,
            output,
        );
        return;
    }
    if node.kind() == "type_alias"
        && let Some(name) = named_child_with_kind(&node, &["type_identifier", "identifier"])
    {
        push_symbol(node, name, source, (parent, "."), "type", depth, output);
        return;
    }
    if node.kind() == "enum_constant"
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(node, name, source, (parent, "."), "variant", depth, output);
        return;
    }
    walk_named_children(node, |child| {
        walk_dart(child, source, parent, depth, output)
    });
}

fn walk_elixir(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if node.kind() == "call"
        && let Some(target) = node.child_by_field_name("target")
    {
        let target_text = one_line(&source_slice(
            source,
            target.start_byte(),
            target.end_byte(),
        ));
        if matches!(
            target_text.as_str(),
            "defmodule" | "defprotocol" | "defimpl"
        ) && let Some(arguments) = named_child_with_kind(&node, &["arguments"])
            && let Some(name) = arguments.named_child(0)
        {
            let kind = match target_text.as_str() {
                "defmodule" => "module",
                "defprotocol" => "protocol",
                _ => "implementation",
            };
            let qualified = push_symbol(node, name, source, (parent, "."), kind, depth, output);
            if let Some(body) = named_child_with_kind(&node, &["do_block"]) {
                walk_elixir(body, source, Some(&qualified), depth + 1, output);
            }
            return;
        }
        if matches!(
            target_text.as_str(),
            "def" | "defp" | "defmacro" | "defmacrop" | "defguard" | "defguardp"
        ) && let Some(arguments) = named_child_with_kind(&node, &["arguments"])
            && let Some(head) = arguments.named_child(0)
            && let Some(name) = elixir_head_name(head)
        {
            push_symbol(
                node,
                name,
                source,
                (parent, "."),
                if target_text.contains("macro") {
                    "macro"
                } else {
                    "function"
                },
                depth,
                output,
            );
            return;
        }
    }
    walk_named_children(node, |child| {
        walk_elixir(child, source, parent, depth, output)
    });
}

fn elixir_head_name(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "call"
        && let Some(target) = node.child_by_field_name("target")
        && target.kind() == "identifier"
    {
        return Some(target);
    }
    if node.kind() == "identifier" {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = elixir_head_name(child) {
            return Some(found);
        }
    }
    None
}

#[derive(Debug)]
enum LeanScopeContext {
    Namespace(Option<String>),
    Section,
}

#[derive(Debug)]
struct LeanScope {
    context: LeanScopeContext,
    symbol_index: Option<usize>,
    visible: bool,
}

#[derive(Debug, Default)]
struct LeanScopes {
    stack: Vec<LeanScope>,
    namespace: Option<String>,
    visible_depth: usize,
}

impl LeanScopes {
    fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    fn depth(&self) -> usize {
        self.visible_depth
    }

    fn push_namespace(&mut self, namespace: String, symbol_index: Option<usize>) {
        let previous = self.namespace.replace(namespace);
        self.stack.push(LeanScope {
            context: LeanScopeContext::Namespace(previous),
            symbol_index,
            visible: true,
        });
        self.visible_depth = self.visible_depth.saturating_add(1);
    }

    fn push_section(&mut self, symbol_index: Option<usize>, visible: bool) {
        self.stack.push(LeanScope {
            context: LeanScopeContext::Section,
            symbol_index,
            visible,
        });
        self.visible_depth = self.visible_depth.saturating_add(usize::from(visible));
    }

    fn close(
        &mut self,
        output: &mut SymbolCollector,
        end_byte: usize,
        end_point: Point,
        all: bool,
    ) {
        while let Some(scope) = self.stack.pop() {
            if let Some(index) = scope.symbol_index
                && let Some(symbol) = output.symbols.get_mut(index)
            {
                symbol.end_byte = end_byte;
                symbol.end_row = end_point.row;
                symbol.end_column = end_point.column;
            }
            self.visible_depth = self
                .visible_depth
                .saturating_sub(usize::from(scope.visible));
            if let LeanScopeContext::Namespace(previous) = scope.context {
                self.namespace = previous;
            }
            if !all {
                break;
            }
        }
    }
}

fn walk_lean(root: Node<'_>, source: &str, output: &mut SymbolCollector) {
    let mut scopes = LeanScopes::default();
    walk_lean_commands(root, source, &mut scopes, output);
    scopes.close(output, root.end_byte(), root.end_position(), true);
}

fn walk_lean_commands(
    node: Node<'_>,
    source: &str,
    scopes: &mut LeanScopes,
    output: &mut SymbolCollector,
) {
    match node.kind() {
        "namespace" => {
            let Some(name) = node.child_by_field_name("name") else {
                return;
            };
            let parent = scopes.namespace();
            let before = output.symbols.len();
            let qualified = push_symbol(
                node,
                name,
                source,
                (parent, "."),
                "namespace",
                scopes.depth(),
                output,
            );
            scopes.push_namespace(qualified, (output.symbols.len() > before).then_some(before));
            return;
        }
        "section" => {
            let name = node.child_by_field_name("name");
            let symbol_index = name.and_then(|name| {
                let before = output.symbols.len();
                push_symbol(
                    node,
                    name,
                    source,
                    (scopes.namespace(), "."),
                    "section",
                    scopes.depth(),
                    output,
                );
                (output.symbols.len() > before).then_some(before)
            });
            scopes.push_section(symbol_index, name.is_some());
            return;
        }
        "end" => {
            scopes.close(output, node.end_byte(), node.end_position(), false);
            return;
        }
        "declaration" => {
            if let Some(declaration) = lean_declaration_child(node) {
                collect_lean_declaration(
                    node,
                    declaration,
                    source,
                    scopes.namespace(),
                    scopes.depth(),
                    output,
                );
            }
            return;
        }
        _ => {}
    }
    walk_named_children(node, |child| {
        walk_lean_commands(child, source, scopes, output)
    });
}

fn lean_declaration_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| lean_declaration_base_kind(child.kind()).is_some())
}

fn collect_lean_declaration(
    range_node: Node<'_>,
    declaration: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    let Some(name) = declaration.child_by_field_name("name") else {
        return;
    };
    let kind = match declaration.kind() {
        "structure" if lean_prefix_has_keyword(declaration, name, source, "class") => "class",
        "inductive" if lean_prefix_has_keyword(declaration, name, source, "class") => {
            "class-inductive"
        }
        other => lean_declaration_base_kind(other).expect("declaration kind was prefiltered"),
    };
    let qualified = push_symbol(range_node, name, source, (parent, "."), kind, depth, output);
    collect_lean_members(
        declaration,
        source,
        &qualified,
        depth.saturating_add(1),
        output,
    );
}

fn collect_lean_members(
    node: Node<'_>,
    source: &str,
    parent: &str,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if output.truncated {
        return;
    }
    let kind = match node.kind() {
        "field" => Some("field"),
        "ctor" | "ctor_alt" => Some("constructor"),
        "where_aux_def" => Some("definition"),
        _ => None,
    };
    if let Some(kind) = kind
        && let Some(name) = node.child_by_field_name("name")
    {
        push_symbol(node, name, source, (Some(parent), "."), kind, depth, output);
        return;
    }
    walk_named_children(node, |child| {
        collect_lean_members(child, source, parent, depth, output)
    });
}

fn lean_declaration_base_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "def" => Some("definition"),
        "theorem" => Some("theorem"),
        "abbrev" => Some("abbrev"),
        "instance" => Some("instance"),
        "axiom" => Some("axiom"),
        "opaque" => Some("opaque"),
        "constant" => Some("constant"),
        "structure" => Some("structure"),
        "inductive" => Some("inductive"),
        _ => None,
    }
}

fn lean_prefix_has_keyword(
    declaration: Node<'_>,
    name: Node<'_>,
    source: &str,
    keyword: &str,
) -> bool {
    source_slice(source, declaration.start_byte(), name.start_byte())
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .any(|token| token == keyword)
}

fn walk_julia(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if node.kind() == "module_definition"
        && let Some(name) = node.child_by_field_name("name")
    {
        let qualified = push_symbol(node, name, source, (parent, "."), "module", depth, output);
        walk_named_children(node, |child| {
            if child != name {
                walk_julia(child, source, Some(&qualified), depth + 1, output);
            }
        });
        return;
    }
    let type_kind = match node.kind() {
        "struct_definition" => "struct",
        "abstract_definition" => "abstract-type",
        "primitive_definition" => "primitive-type",
        _ => "",
    };
    if !type_kind.is_empty()
        && let Some(head) = named_child_with_kind(&node, &["type_head"])
        && let Some(name) = descendant_with_kind(head, "identifier")
    {
        let qualified = push_symbol(node, name, source, (parent, "."), type_kind, depth, output);
        if node.kind() == "struct_definition" {
            walk_named_children(node, |child| {
                if child.kind() == "typed_expression"
                    && let Some(field) = named_child_with_kind(&child, &["identifier"])
                {
                    push_symbol(
                        child,
                        field,
                        source,
                        (Some(&qualified), "."),
                        "field",
                        depth + 1,
                        output,
                    );
                }
            });
        }
        return;
    }
    if node.kind() == "function_definition"
        && let Some(signature_node) = named_child_with_kind(&node, &["signature"])
        && let Some(name) = julia_callable_name(signature_node)
    {
        push_symbol(node, name, source, (parent, "."), "function", depth, output);
        return;
    }
    if node.kind() == "assignment"
        && let Some(left) = node.named_child(0)
        && left.kind() == "call_expression"
        && let Some(name) = julia_callable_name(left)
    {
        push_symbol(node, name, source, (parent, "."), "function", depth, output);
        return;
    }
    if node.kind() == "macro_definition"
        && let Some(signature_node) = named_child_with_kind(&node, &["signature"])
        && let Some(name) = julia_callable_name(signature_node)
    {
        let text = source_slice(source, name.start_byte(), name.end_byte());
        push_symbol_name(
            node,
            (
                &format!("@{}", one_line(&text)),
                Some(name.start_position()),
            ),
            source,
            (parent, "."),
            "macro",
            depth,
            output,
        );
        return;
    }
    walk_named_children(node, |child| {
        walk_julia(child, source, parent, depth, output)
    });
}

fn julia_callable_name(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "call_expression" {
        return node
            .named_child(0)
            .and_then(|function| match function.kind() {
                "identifier" | "field_expression" => Some(function),
                _ => julia_callable_name(function),
            });
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = julia_callable_name(child) {
            return Some(found);
        }
    }
    None
}

fn descendant_type_name(node: Node<'_>) -> Option<Node<'_>> {
    if matches!(node.kind(), "type_identifier" | "identifier") {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = descendant_type_name(child) {
            return Some(found);
        }
    }
    None
}

fn is_program_level(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "program" {
            return true;
        }
        if matches!(
            parent.kind(),
            "statement_block" | "class_body" | "function_declaration" | "method_definition"
        ) {
            return false;
        }
        current = parent.parent();
    }
    false
}

fn walk_named_children(node: Node<'_>, mut visit: impl FnMut(Node<'_>)) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child);
    }
}

fn declarator_name(node: Node<'_>) -> Option<Node<'_>> {
    if matches!(node.kind(), "identifier" | "field_identifier") || cpp_path_name(node) {
        return Some(node);
    }
    if let Some(declarator) = node.child_by_field_name("declarator")
        && let Some(found) = declarator_name(declarator)
    {
        return Some(found);
    }
    descendant_name(node)
}

fn descendant_name(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return Some(child);
        }
        if let Some(found) = descendant_name(child) {
            return Some(found);
        }
    }
    None
}

fn descendant_with_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
        if let Some(found) = descendant_with_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

fn enclosing_class_like(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(
            parent.kind(),
            "class_specifier" | "struct_specifier" | "union_specifier"
        ) {
            return Some(parent);
        }
        current = parent.parent();
    }
    None
}

fn inspect_tree(root: Node<'_>) -> Result<usize, usize> {
    let mut cursor = root.walk();
    let mut depth = 0;
    let mut defects = 0;
    loop {
        let node = cursor.node();
        if depth > MAX_SYNTAX_DEPTH {
            return Err(depth);
        }
        defects += usize::from(node.is_error() || node.is_missing());

        if cursor.goto_first_child() {
            depth += 1;
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return Ok(defects);
            }
            depth -= 1;
        }
    }
}

fn walk_python(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    parent_is_class: bool,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if parent.is_none() && node.kind() == "assignment" {
        let mut assignment = node;
        loop {
            if let Some(left) = assignment.child_by_field_name("left") {
                add_python_bindings(node, left, source, depth, output);
            }
            match assignment.child_by_field_name("right") {
                Some(right) if right.kind() == "assignment" => assignment = right,
                _ => break,
            }
        }
        return;
    }
    if parent.is_none()
        && node.kind() == "type_alias_statement"
        && let Some(name_node) = node.child_by_field_name("name")
    {
        push_symbol(
            node,
            name_node,
            source,
            (None, "."),
            "binding",
            depth,
            output,
        );
        return;
    }
    if node.kind() == "decorated_definition" {
        if let Some(definition) =
            named_child_with_kind(&node, &["class_definition", "function_definition"])
        {
            add_python_definition(
                definition,
                Some((node.start_byte(), node.start_position())),
                source,
                parent,
                parent_is_class,
                depth,
                output,
            );
        }
        return;
    }
    if matches!(node.kind(), "class_definition" | "function_definition") {
        add_python_definition(node, None, source, parent, parent_is_class, depth, output);
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_python(child, source, parent, parent_is_class, depth, output);
    }
}

fn add_python_bindings(
    assignment: Node<'_>,
    target: Node<'_>,
    source: &str,
    depth: usize,
    output: &mut SymbolCollector,
) {
    add_pattern_bindings(assignment, target, source, None, "binding", depth, output);
}

fn add_python_definition(
    node: Node<'_>,
    range_start: Option<(usize, Point)>,
    source: &str,
    parent: Option<&str>,
    parent_is_class: bool,
    depth: usize,
    output: &mut SymbolCollector,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = source_slice(source, name_node.start_byte(), name_node.end_byte()).into_owned();
    let (path, qualified, legacy_qualified_name) = qualified_names(parent, &name, ".", output);
    let kind = if node.kind() == "class_definition" {
        "class"
    } else if parent_is_class {
        "method"
    } else {
        "function"
    };
    let (start_byte, start_position) =
        range_start.unwrap_or_else(|| (node.start_byte(), node.start_position()));
    output.push(Symbol {
        kind,
        path,
        qualified_name: qualified.clone(),
        legacy_qualified_name,
        signature: signature(node, source, start_byte),
        name_position: Some((
            name_node.start_position().row,
            name_node.start_position().column,
        )),
        start_byte,
        end_byte: node.end_byte(),
        start_row: start_position.row,
        start_column: start_position.column,
        end_row: node.end_position().row,
        end_column: node.end_position().column,
        depth,
    });
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            walk_python(
                child,
                source,
                Some(&qualified),
                node.kind() == "class_definition",
                depth + 1,
                output,
            );
        }
    }
}

fn walk_rust(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut SymbolCollector,
) {
    if node.kind() == "impl_item" {
        let type_name = node
            .child_by_field_name("type")
            .map(|part| rust_impl_owner(&source_slice(source, part.start_byte(), part.end_byte())))
            .unwrap_or_else(|| "impl".into());
        let qualified = qualify(parent, &type_name, "::");
        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.named_children(&mut cursor) {
                walk_rust(child, source, Some(&qualified), depth + 1, output);
            }
        }
        return;
    }

    let kind = match node.kind() {
        "struct_item" => Some("struct"),
        "enum_item" => Some("enum"),
        "trait_item" => Some("trait"),
        "type_item" => Some("type"),
        "function_item" | "function_signature_item" => {
            let owner = node.parent().and_then(|body| {
                if body.kind() == "declaration_list" {
                    body.parent()
                } else {
                    Some(body)
                }
            });
            Some(
                if owner.is_some_and(|owner| matches!(owner.kind(), "impl_item" | "trait_item")) {
                    "method"
                } else {
                    "function"
                },
            )
        }
        "const_item" => Some("const"),
        "static_item" => Some("static"),
        "mod_item" => Some("module"),
        "field_declaration" if parent.is_some() => Some("field"),
        "enum_variant" if parent.is_some() => Some("variant"),
        _ => None,
    };
    if let Some(kind) = kind {
        let name_node = node.child_by_field_name("name");
        if let Some(name_node) = name_node {
            let name =
                source_slice(source, name_node.start_byte(), name_node.end_byte()).into_owned();
            let (path, qualified, legacy_qualified_name) =
                qualified_names(parent, &name, "::", output);
            let (start_byte, start_position) = rust_attached_start(node, source);
            output.push(Symbol {
                kind,
                path,
                qualified_name: qualified.clone(),
                legacy_qualified_name,
                // Attached docs/attributes belong to `show`, not the compact signature.
                signature: signature(node, source, node.start_byte()),
                name_position: Some((
                    name_node.start_position().row,
                    name_node.start_position().column,
                )),
                start_byte,
                end_byte: node.end_byte(),
                start_row: start_position.row,
                start_column: start_position.column,
                end_row: node.end_position().row,
                end_column: node.end_position().column,
                depth,
            });
            if !matches!(node.kind(), "function_item" | "function_signature_item") {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if child.id() != name_node.id() {
                        walk_rust(child, source, Some(&qualified), depth + 1, output);
                    }
                }
            }
            return;
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_rust(child, source, parent, depth, output);
    }
}

fn signature(node: Node<'_>, source: &str, start_byte: usize) -> String {
    let end = node
        .child_by_field_name("body")
        .map_or_else(|| first_line_end(node, source), |body| body.start_byte());
    one_line(&source_slice(source, start_byte, end))
}

fn rust_impl_owner(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut generic_depth = 0usize;
    for character in value.chars() {
        match character {
            '<' => generic_depth = generic_depth.saturating_add(1),
            '>' if generic_depth > 0 => generic_depth -= 1,
            _ if generic_depth == 0 => output.push(character),
            _ => {}
        }
    }
    let output = one_line(&output);
    if output.is_empty() {
        one_line(value)
    } else {
        output
    }
}

fn rust_attached_start(node: Node<'_>, source: &str) -> (usize, Point) {
    let mut start_node = node;
    let mut previous = node.prev_named_sibling();
    while let Some(candidate) = previous {
        let text = source_slice(source, candidate.start_byte(), candidate.end_byte());
        let attachable = candidate.kind() == "attribute_item"
            || (candidate.kind() == "line_comment" && text.trim_start().starts_with("///"))
            || (candidate.kind() == "block_comment" && text.trim_start().starts_with("/**"));
        if !attachable {
            break;
        }
        let gap = source_slice(source, candidate.end_byte(), start_node.start_byte());
        if gap.bytes().filter(|byte| *byte == b'\n').count() > 1 {
            break;
        }
        start_node = candidate;
        previous = candidate.prev_named_sibling();
    }
    (start_node.start_byte(), start_node.start_position())
}

fn first_line_end(node: Node<'_>, source: &str) -> usize {
    let slice = source_slice(source, node.start_byte(), node.end_byte());
    slice
        .find('\n')
        .map_or(node.end_byte(), |offset| node.start_byte() + offset)
}

fn named_child_with_kind<'tree>(node: &Node<'tree>, kinds: &[&str]) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn qualify(parent: Option<&str>, name: &str, separator: &str) -> String {
    qualified_path(parent, name, separator).canonical()
}

fn qualified_names(
    parent: Option<&str>,
    name: &str,
    separator: &str,
    output: &SymbolCollector,
) -> (SymbolPath, String, String) {
    let path = qualified_path(parent, name, separator);
    let qualified = path.canonical();
    let legacy_qualified_name = match parent.filter(|parent| !parent.is_empty()) {
        Some(parent) => {
            let legacy_parent = output
                .symbols
                .iter()
                .rev()
                .find(|symbol| symbol.qualified_name == parent)
                .map(|symbol| symbol.legacy_qualified_name.clone())
                .or_else(|| {
                    SymbolPath::parse_canonical(parent).map(|path| path.legacy_code(separator))
                })
                .unwrap_or_else(|| parent.to_owned());
            format!("{legacy_parent}{separator}{name}")
        }
        None => name.to_owned(),
    };
    (path, qualified, legacy_qualified_name)
}

fn qualified_path(parent: Option<&str>, name: &str, separator: &str) -> SymbolPath {
    let parent = parent.map_or_else(SymbolPath::default, |parent| {
        SymbolPath::parse_canonical(parent).expect("internal parent symbol path must be canonical")
    });
    let names = if separator.is_empty() {
        vec![name.to_owned()]
    } else {
        name.split(separator)
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    };
    parent.extend_names(names)
}

#[cfg(test)]
mod parse_completeness_tests {
    use super::inspect_tree;
    use tree_sitter::Parser;

    #[test]
    fn defects_are_counted_even_inside_function_bodies() {
        let source = "def broken():\n    value = (\n";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("python grammar");
        let tree = parser.parse(source.as_bytes(), None).expect("tree");
        assert!(inspect_tree(tree.root_node()).expect("bounded tree") > 0);
    }
}

#[cfg(test)]
mod rust_navigation_tests {
    use super::rust_impl_owner;

    #[test]
    fn impl_owner_omits_generic_arguments_from_navigation_names() {
        assert_eq!(rust_impl_owner("SourcePositions<'a>"), "SourcePositions");
        assert_eq!(rust_impl_owner("crate::Cache<K, Vec<V>>"), "crate::Cache");
        assert_eq!(rust_impl_owner("Plain"), "Plain");
    }
}

#[cfg(test)]
mod lean_navigation_tests {
    use std::path::Path;

    use super::parse_source_symbols;
    use crate::language::Language;

    #[test]
    fn anonymous_sections_do_not_create_invisible_outline_depth() {
        let source = "namespace Demo\nsection\ndef value : Nat := 1\nend\nend Demo\n";
        let (symbols, defects) =
            parse_source_symbols(Path::new("Demo.lean"), Language::Lean, source).unwrap();
        assert_eq!(defects, 0);
        assert_eq!(
            symbols
                .iter()
                .map(|symbol| (symbol.qualified_name.as_str(), symbol.depth))
                .collect::<Vec<_>>(),
            vec![("Demo", 0), ("Demo::value", 1)]
        );
    }
}

#[cfg(test)]
mod symbol_limit_tests {
    use std::path::Path;

    use super::{MAX_CODE_SYMBOLS, parse_source_symbols_state};
    use crate::language::Language;

    #[test]
    fn code_symbol_collection_is_bounded_and_reports_truncation() {
        let source = (0..MAX_CODE_SYMBOLS + 10)
            .map(|index| format!("fn f{index}() {{}}\n"))
            .collect::<String>();
        let (symbols, defects, truncated) =
            parse_source_symbols_state(Path::new("many.rs"), Language::Rust, &source).unwrap();
        assert_eq!(defects, 0);
        assert_eq!(symbols.len(), MAX_CODE_SYMBOLS);
        assert!(truncated);
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    fn names(language: Language, source: &str) -> Vec<String> {
        let (symbols, defects) =
            parse_source_symbols(Path::new("fixture"), language, source).unwrap();
        assert_eq!(defects, 0, "{language:?}: {source}");
        symbols.into_iter().map(|s| s.qualified_name).collect()
    }

    #[test]
    fn scala_identifier_lists_bind_top_level_and_member_val_var_names() {
        assert_eq!(
            names(
                Language::Scala,
                "val one, two = rhs\nvar three, four: Int = rhs\nobject O { val five, six = rhs; var seven, eight: Int = rhs; val ordinary = rhs; val (nine, ten) = rhs }"
            ),
            [
                "one",
                "two",
                "three",
                "four",
                "O",
                "O::five",
                "O::six",
                "O::seven",
                "O::eight",
                "O::ordinary",
                "O::nine",
                "O::ten"
            ]
        );
    }

    #[test]
    fn r_backtick_identifiers_use_r_escapes_and_remain_literal_segments() {
        for (source, name) in [
            (r#"`a\`b` <- function() {}"#, "a`b"),
            (r#"`a\\b` <- function() {}"#, r"a\b"),
            (r#"`a\x60b` <- function() {}"#, "a`b"),
            (r#"`a\\\`b` <- function() {}"#, r"a\`b"),
            (r#"(function() {}) -> `a\\b`"#, r"a\b"),
            (r#"`plain.dot` <- function() {}"#, "plain.dot"),
            (r#""a`b" <- function() {}"#, "a`b"),
            (r#"plain.dot <- function() {}"#, "plain.dot"),
        ] {
            assert_eq!(
                names(Language::R, source),
                [SymbolPath::from_names([name.to_owned()]).canonical()],
                "{source}"
            );
        }
        assert_eq!(
            document::decode_quoted_scalar(r#"`a\u0062`"#, Language::R),
            None
        );
        assert_eq!(
            document::decode_quoted_scalar(r#""a\u0062""#, Language::R).as_deref(),
            Some("ab")
        );
    }

    #[test]
    fn cpp_destructor_tokens_ignore_spacing_comments_and_keep_name_coordinates() {
        let source = "struct C { C(); ~ C(); };\nstruct D { D(); ~ /*between*/ D() {} };\nnamespace N { struct E { ~E(); }; E::~ /*between*/ E() {} }\n";
        for language in [Language::Cpp, Language::Cuda] {
            let (symbols, defects) =
                parse_source_symbols(Path::new("fixture"), language, source).unwrap();
            assert_eq!(defects, 0);
            let destructors = symbols
                .iter()
                .filter(|s| s.qualified_name.contains('~'))
                .collect::<Vec<_>>();
            assert_eq!(
                destructors
                    .iter()
                    .map(|s| s.qualified_name.as_str())
                    .collect::<Vec<_>>(),
                [
                    r#"C::["~C"]"#,
                    r#"D::["~D"]"#,
                    r#"N::E::["~E"]"#,
                    r#"N::E::["~E"]"#
                ]
            );
            for symbol in destructors {
                assert_eq!(symbol.kind, "method");
                let (row, column) = symbol
                    .name_position
                    .expect("comments must not hide name position");
                assert!(matches!(
                    source.lines().nth(row).unwrap().as_bytes()[column],
                    b'C' | b'D' | b'E'
                ));
            }
        }
    }

    #[test]
    fn cpp_direct_template_names_keep_atomic_spelling_and_containing_ancestry() {
        for (prefix, declaration, suffix, atom) in [
            (
                "namespace N { struct Tag {}; template<class T> struct Box; template<> ",
                "struct Box<N::Tag> { Box() {} ~Box() {} void run() {} }",
                "; }",
                "Box<N::Tag>",
            ),
            (
                "namespace N { struct Tag {}; template<class T> struct Box; } template<> ",
                "struct N::Box<N::Tag> { Box() {} ~Box() {} void run() {} }",
                ";",
                "Box<N::Tag>",
            ),
            (
                "namespace N { struct Tag {}; template<class T> struct Box; template<> ",
                "struct Box< ::N::Tag > { Box() {} ~Box() {} void run() {} }",
                "; }",
                "Box< ::N::Tag >",
            ),
            (
                "namespace N { template<int> struct Box; template<> ",
                r#"struct Box<R"(a  b::c)"[0]> { Box() {} ~Box() {} void run() {} }"#,
                "; }",
                r#"Box<R"(a  b::c)"[0]>"#,
            ),
        ] {
            let source = format!("{prefix}{declaration}{suffix}");
            let owner = SymbolPath::from_names(["N".to_owned(), atom.to_owned()]);
            for language in [Language::Cpp, Language::Cuda] {
                let (symbols, defects) =
                    parse_source_symbols(Path::new("fixture"), language, &source).unwrap();
                assert_eq!(defects, 0, "{source}");
                let container = symbols
                    .iter()
                    .find(|symbol| symbol.path == owner)
                    .expect("atomic specialization");
                assert_eq!(
                    (container.start_byte, container.end_byte),
                    (prefix.len(), prefix.len() + declaration.len())
                );
                assert_eq!(
                    container.name_position,
                    Some((0, source.find(atom).unwrap()))
                );
                for (name, kind, body) in [
                    ("Box", "constructor", "Box() {}"),
                    ("~Box", "method", "~Box() {}"),
                    ("run", "method", "void run() {}"),
                ] {
                    let symbol = symbols
                        .iter()
                        .find(|symbol| symbol.path == owner.child_name(name))
                        .expect("specialization member");
                    assert_eq!(symbol.kind, kind);
                    assert_eq!(&source[symbol.start_byte..symbol.end_byte], body);
                }
            }
        }
        for (opening, closing, scopes) in [
            ("namespace Outer { namespace N {", "} }", vec!["Outer", "N"]),
            ("namespace Outer::N {", "}", vec!["Outer", "N"]),
            ("namespace Outer /*scope*/ :: N {", "}", vec!["Outer", "N"]),
            (
                "namespace Outer::Middle /*scope*/ :: N {",
                "}",
                vec!["Outer", "Middle", "N"],
            ),
        ] {
            let atom = format!("Box<{}::Tag>", scopes.join("::"));
            let source = format!(
                "{opening} struct Tag {{}}; template<class T> struct Box; template<> struct {atom} {{ struct Inner {{ Inner() {{}} ~Inner() {{}} }}; }}; {closing}"
            );
            for language in [Language::Cpp, Language::Cuda] {
                let owner = SymbolPath::from_names(scopes.iter().map(|s| (*s).to_owned()))
                    .child_name(&atom)
                    .child_name("Inner");
                let (symbols, defects) =
                    parse_source_symbols(Path::new("fixture"), language, &source).unwrap();
                assert_eq!(defects, 0);
                assert!(symbols.iter().any(|symbol| symbol.path == owner));
                assert!(
                    symbols
                        .iter()
                        .any(|symbol| symbol.path == owner.child_name("Inner")
                            && symbol.kind == "constructor")
                );
                assert!(
                    symbols
                        .iter()
                        .any(|symbol| symbol.path == owner.child_name("~Inner")
                            && symbol.kind == "method")
                );
            }
        }
        // The same direct-template identity boundary applies to callable declarators.
        for (source, path, body) in [
            (
                "namespace N { struct Tag {}; template<class T> void f(); template<> void f<N::Tag>() {} }",
                r#"N::["f<N::Tag>"]"#,
                "void f<N::Tag>() {}",
            ),
            (
                "namespace N { struct Tag {}; template<class T> void f(); } template<> void N::f<N::Tag>() {}",
                r#"N::["f<N::Tag>"]"#,
                "void N::f<N::Tag>() {}",
            ),
            (
                "namespace N { template<int> void f(); template<> void f<R\"(a  b::c)\"[0]>() {} }",
                r#"N::["f<R\"(a  b::c)\"[0]>"]"#,
                "void f<R\"(a  b::c)\"[0]>() {}",
            ),
        ] {
            for language in [Language::Cpp, Language::Cuda] {
                let (symbols, defects) =
                    parse_source_symbols(Path::new("fixture"), language, source).unwrap();
                assert_eq!(defects, 0);
                let symbol = symbols
                    .iter()
                    .find(|symbol| symbol.qualified_name == path)
                    .expect("atomic callable specialization");
                assert_eq!(&source[symbol.start_byte..symbol.end_byte], body);
                assert_eq!(
                    symbol.name_position,
                    Some((0, source.find(body).unwrap() + body.find('f').unwrap()))
                );
                assert!(symbols.iter().any(|symbol| symbol.qualified_name == "N::f"));
            }
        }
        for language in [Language::Cpp, Language::Cuda] {
            let source = "template<class T> struct Box { Box<T>() {} };";
            let (symbols, defects) =
                parse_source_symbols(Path::new("fixture"), language, source).unwrap();
            assert_eq!(defects, 0);
            let constructor = symbols
                .iter()
                .find(|symbol| symbol.qualified_name == r#"Box::["Box<T>"]"#)
                .expect("atomic constructor template-id");
            assert_eq!(constructor.kind, "constructor");
            assert_eq!(
                &source[constructor.start_byte..constructor.end_byte],
                "Box<T>() {}"
            );
            assert_eq!(
                constructor.name_position,
                Some((0, source.find("Box<T>").unwrap()))
            );
        }
    }

    #[test]
    fn cpp_global_anchor_is_not_containing_ancestry_or_template_argument_punctuation() {
        for language in [Language::Cpp, Language::Cuda] {
            for first in ["::N::C::~C() {}", "C::~C() {}"] {
                let source = format!("namespace N {{ {first} namespace N {{ C::~C() {{}} }} }}");
                let (symbols, defects) =
                    parse_source_symbols(Path::new("fixture"), language, &source).unwrap();
                assert_eq!(defects, 0);
                let members = symbols
                    .iter()
                    .filter(|symbol| symbol.kind == "method")
                    .collect::<Vec<_>>();
                assert_eq!(
                    members
                        .iter()
                        .map(|s| s.qualified_name.as_str())
                        .collect::<Vec<_>>(),
                    [r#"N::C::["~C"]"#, r#"N::N::C::["~C"]"#]
                );
                assert_eq!(&source[members[0].start_byte..members[0].end_byte], first);
                assert_eq!(
                    members[0].name_position,
                    Some((0, source.find(first).unwrap() + first.rfind('C').unwrap()))
                );
            }
            let source =
                "namespace N { ::N::C::C() {} ::N::C::~ /*leaf*/ C() {} void ::N::C::run() {} }";
            let (symbols, defects) =
                parse_source_symbols(Path::new("fixture"), language, source).unwrap();
            assert_eq!(defects, 0);
            assert_eq!(
                symbols
                    .iter()
                    .filter(|symbol| matches!(symbol.kind, "constructor" | "method"))
                    .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
                    .collect::<Vec<_>>(),
                [
                    ("N::C::C", "constructor"),
                    (r#"N::C::["~C"]"#, "method"),
                    ("N::C::run", "method")
                ]
            );
            // Containers and their descendants use the same global anchor as callables.
            let source = "namespace N { struct C; struct ::N::C { C() {} ~C() {} }; struct Tag {}; template<class T> struct Box; template<> struct ::N::Box<N::Tag> { Box() {} ~Box() {} }; }";
            let (symbols, defects) =
                parse_source_symbols(Path::new("fixture"), language, source).unwrap();
            assert_eq!(defects, 0);
            assert!(
                symbols
                    .iter()
                    .all(|symbol| !symbol.qualified_name.starts_with("N::N::"))
            );
            assert!(symbols.iter().any(|symbol| symbol.qualified_name
                == r#"N::["Box<N::Tag>"]::Box"#
                && symbol.kind == "constructor"));
            assert!(
                symbols.iter().any(
                    |symbol| symbol.qualified_name == "N::C::C" && symbol.kind == "constructor"
                )
            );
        }
    }

    #[test]
    fn cpp_qualified_destructors_use_ast_paths_at_each_depth_and_trivia_boundary() {
        for language in [Language::Cpp, Language::Cuda] {
            for scopes in [vec!["Plain"], vec!["N", "C"], vec!["N", "Inner", "D"]] {
                let class = scopes.last().unwrap();
                for (separator, tilde_gap) in [
                    ("::", ""),
                    (" :: ", " "),
                    ("::", " /*leaf*/ "),
                    (" /*scope*/ :: /*next*/ ", ""),
                    ("\n:: // next scope\n", " // class name\n"),
                ] {
                    let source = format!("{}::~{tilde_gap}{class}() {{}}", scopes.join(separator));
                    let expected = SymbolPath::from_names(
                        scopes
                            .iter()
                            .map(|s| (*s).to_owned())
                            .chain([format!("~{class}")]),
                    );
                    let (symbols, defects) =
                        parse_source_symbols(Path::new("fixture"), language, &source).unwrap();
                    assert_eq!(defects, 0, "{source}");
                    assert_eq!(symbols.len(), 1, "{source}");
                    let symbol = &symbols[0];
                    assert_eq!(symbol.path, expected, "{source}");
                    assert_eq!(symbol.kind, "method", "{source}");
                    assert_eq!((symbol.start_byte, symbol.end_byte), (0, source.len()));
                    let offset = source.rfind(class).unwrap();
                    let prefix = &source[..offset];
                    let row = prefix.bytes().filter(|b| *b == b'\n').count();
                    let column = prefix.rsplit('\n').next().unwrap().len();
                    assert_eq!(symbol.name_position, Some((row, column)), "{source}");
                }
            }
        }
    }

    #[test]
    fn cpp_qualified_names_preserve_members_templates_operators_and_literal_tokens() {
        for (source, names, kind, name_token) in [
            ("::N::C::~C() {}", vec!["N", "C", "~C"], "method", "C"),
            (
                "void N::C::run() {}",
                vec!["N", "C", "run"],
                "method",
                "run",
            ),
            (
                "void N /*x*/ :: C /*y*/ :: run() {}",
                vec!["N", "C", "run"],
                "method",
                "run",
            ),
            ("N::C::C() {}", vec!["N", "C", "C"], "constructor", "C"),
            (
                "N /*x*/ :: C /*y*/ :: C() {}",
                vec!["N", "C", "C"],
                "constructor",
                "C",
            ),
            (
                "template<class T> N::Box< T >::~ /*leaf*/ Box() {}",
                vec!["N", "Box< T >", "~Box"],
                "method",
                "Box",
            ),
            (
                "template<class T> N::Box< T >::Box() {}",
                vec!["N", "Box< T >", "Box"],
                "constructor",
                "Box",
            ),
            (
                "template<> void N::Box< unsigned int >::run() {}",
                vec!["N", "Box< unsigned int >", "run"],
                "method",
                "run",
            ),
            (
                "template<> void N::Box< N::Tag >::run() {}",
                vec!["N", "Box< N::Tag >", "run"],
                "method",
                "run",
            ),
            (
                r#"template<> void N::ValueBox< ("a  b"[0]) >::run() {}"#,
                vec!["N", r#"ValueBox< ("a  b"[0]) >"#, "run"],
                "method",
                "run",
            ),
            (
                r#"template<> void N::ValueBox< (R"(a  b /*literal*/)"[0]) >::run() {}"#,
                vec!["N", r#"ValueBox< (R"(a  b /*literal*/)"[0]) >"#, "run"],
                "method",
                "run",
            ),
            (
                "template<> void N::ValueBox< (' ') >::run() {}",
                vec!["N", "ValueBox< (' ') >", "run"],
                "method",
                "run",
            ),
            (
                "template<> void N::ValueBox< (1 + +2) >::run() {}",
                vec!["N", "ValueBox< (1 + +2) >", "run"],
                "method",
                "run",
            ),
            (
                "void N::C::operator()() const {}",
                vec!["N", "C", "operator()"],
                "method",
                "operator",
            ),
            (
                "void N /*x*/ :: C :: operator /*op*/ ()() const {}",
                vec!["N", "C", "operator()"],
                "method",
                "operator",
            ),
            (
                "void* N::C::operator /*op*/ new(unsigned long) { throw 0; }",
                vec!["N", "C", "operator new"],
                "method",
                "operator",
            ),
            (
                "N::C::operator N::Tag() const { return {}; }",
                vec!["N", "C", "operator N::Tag() const"],
                "method",
                "operator",
            ),
            // Retain the native grammar's existing conversion-name extent and type spelling.
            (
                "N::C::operator const char*() const { return nullptr; }",
                vec!["N", "C", "operator const char*() const"],
                "method",
                "operator",
            ),
        ] {
            for language in [Language::Cpp, Language::Cuda] {
                let (symbols, defects) =
                    parse_source_symbols(Path::new("fixture"), language, source).unwrap();
                assert_eq!(defects, 0, "{source}");
                assert_eq!(symbols.len(), 1, "{source}");
                let symbol = &symbols[0];
                let path = SymbolPath::from_names(names.iter().map(|s| (*s).to_owned()));
                assert_eq!(symbol.path, path, "{source}");
                assert_eq!(symbol.qualified_name, path.canonical());
                assert_eq!(symbol.legacy_qualified_name, names.join("::"));
                assert_eq!(symbol.kind, kind, "{source}");
                assert_eq!(
                    symbol.name_position,
                    Some((0, source.rfind(name_token).unwrap())),
                    "{source}"
                );
                let start = if source.starts_with("template") {
                    source.find("> ").unwrap() + 2
                } else {
                    0
                };
                assert_eq!(
                    (symbol.start_byte, symbol.end_byte),
                    (start, source.len()),
                    "{source}"
                );
            }
        }
        // Enclosing specialized-class ownership comes from AST names too.
        let source = "namespace N { template<class T> struct Box; } template<> struct N::Box<int> { Box() {} ~Box() {} void run() {} };";
        for language in [Language::Cpp, Language::Cuda] {
            let (symbols, defects) =
                parse_source_symbols(Path::new("fixture"), language, source).unwrap();
            assert_eq!(defects, 0);
            let members = symbols
                .iter()
                .filter(|s| matches!(s.kind, "constructor" | "method"))
                .map(|s| (s.qualified_name.as_str(), s.kind))
                .collect::<Vec<_>>();
            assert_eq!(
                members,
                [
                    (r#"N::["Box<int>"]::Box"#, "constructor"),
                    (r#"N::["Box<int>"]::["~Box"]"#, "method"),
                    (r#"N::["Box<int>"]::run"#, "method"),
                ]
            );
        }
    }

    #[test]
    fn cpp_destructors_keep_tilde_and_are_not_constructor_overloads() {
        let source = "struct C { C(); C(int); virtual ~C() noexcept; void ordinary(); }; struct D { D() {} ~D() {} }; namespace N { struct E { E(); ~E(); }; E::~E() = default; } template<class T> struct Box { Box() = default; ~Box() = default; };";
        for language in [Language::Cpp, Language::Cuda] {
            let (symbols, defects) =
                parse_source_symbols(Path::new("fixture"), language, source).unwrap();
            assert_eq!(defects, 0);
            let rows = symbols
                .iter()
                .filter(|s| matches!(s.kind, "constructor" | "method"))
                .map(|s| (s.qualified_name.as_str(), s.kind))
                .collect::<Vec<_>>();
            assert_eq!(
                rows,
                [
                    ("C::C", "constructor"),
                    ("C::C", "constructor"),
                    (r#"C::["~C"]"#, "method"),
                    ("C::ordinary", "method"),
                    ("D::D", "constructor"),
                    (r#"D::["~D"]"#, "method"),
                    ("N::E::E", "constructor"),
                    (r#"N::E::["~E"]"#, "method"),
                    (r#"N::E::["~E"]"#, "method"),
                    ("Box::Box", "constructor"),
                    (r#"Box::["~Box"]"#, "method")
                ]
            );
        }
    }

    #[test]
    fn native_text_budget_includes_owned_path_segments() {
        let owner = "A".repeat(4096);
        let source = format!(
            "struct {owner} {{ {} }};",
            (0..600)
                .map(|i| format!("int field{i};"))
                .collect::<String>()
        );
        let (symbols, defects, truncated) =
            parse_source_symbols_state(Path::new("fixture.cpp"), Language::Cpp, &source).unwrap();
        assert_eq!(defects, 0);
        assert!(truncated);
        assert!(symbols.len() < 601);
        assert!(symbols.iter().map(Symbol::text_bytes).sum::<usize>() <= MAX_SYMBOL_TEXT_BYTES);
    }

    #[test]
    fn go_declaration_names_exclude_punctuation_and_initializers() {
        assert_eq!(
            names(
                Language::Go,
                "package p\nvar one, two = rhs1, rhs2\nconst three, four = 3, 4\n"
            ),
            ["one", "two", "three", "four"]
        );
    }

    #[test]
    fn php_anonymous_namespace_keeps_global_declarations() {
        assert_eq!(
            names(
                Language::Php,
                "<?php\nnamespace Named { function visible() {} }\nnamespace { function global_fn() {} class GlobalClass {} }\n"
            ),
            ["Named", "Named::visible", "global_fn", "GlobalClass"]
        );
        assert_eq!(
            names(
                Language::Php,
                "<?php\nnamespace Named; function visible() {}"
            ),
            ["Named", "Named::visible"]
        );
    }

    #[test]
    fn python_chains_and_nested_patterns_bind_only_target_names() {
        for source in [
            "one = (two, *three) = value",
            "one = [two, three] = value",
            "one, (two, three) = value",
            "(one, two, three) = value",
        ] {
            assert_eq!(
                names(Language::Python, source),
                ["one", "two", "three"],
                "{source}"
            );
        }
        assert_eq!(
            names(
                Language::Python,
                "obj.attr = index[slot] = one = value\ndef f():\n    local = rhs\n"
            ),
            ["one", "f"]
        );
    }

    #[test]
    fn c_family_keeps_all_field_and_function_declarators() {
        for language in [Language::C, Language::Cpp, Language::Cuda] {
            assert_eq!(
                names(
                    language,
                    "struct Pair { int one, two; }; int first(void), second(void);"
                ),
                ["Pair", "Pair::one", "Pair::two", "first", "second"]
            );
        }
    }

    #[test]
    fn ecmascript_patterns_do_not_invent_key_or_rhs_bindings() {
        for language in [Language::JavaScript, Language::TypeScript] {
            assert_eq!(
                names(
                    language,
                    "const {one, key: [two = fallback, ...three]} = rhs;"
                ),
                ["one", "two", "three"]
            );
            assert_eq!(
                names(language, "const [one, {key: two}, ...three] = rhs;"),
                ["one", "two", "three"]
            );
            assert_eq!(
                names(
                    language,
                    "const one = 1, two = 2; function f() { let local = rhs; }"
                ),
                ["one", "two", "f"]
            );
        }
    }

    #[test]
    fn scala_swift_and_kotlin_tuple_bindings_keep_each_declared_name() {
        for (language, source, ordinary) in [
            (
                Language::Scala,
                "val (one, (_, two)) = rhs",
                "val one = 1\nval two = 2",
            ),
            (
                Language::Swift,
                "let (one, (_, two)) = rhs",
                "let one = 1\nlet two = 2",
            ),
            (
                Language::Kotlin,
                "val (one, _, two) = rhs",
                "val one = 1\nval two = 2",
            ),
        ] {
            assert_eq!(names(language, source), ["one", "two"]);
            assert_eq!(names(language, ordinary), ["one", "two"]);
        }
    }

    #[test]
    fn typed_tuple_bindings_do_not_harvest_type_names() {
        let source = "val (one: Int, two: Int) = rhs";
        let mut parser = Language::Scala.parser(Path::new("fixture.scala")).unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(
            names(Language::Scala, source),
            ["one", "two"],
            "{}",
            tree.root_node().to_sexp()
        );
        assert_eq!(
            names(Language::Swift, "let (one, two): (Int, Int) = rhs"),
            ["one", "two"]
        );
        assert_eq!(
            names(Language::Kotlin, "val (one: Int, two: Int) = rhs"),
            ["one", "two"]
        );
        assert_eq!(
            names(
                Language::Go,
                "package p\nvar one, _ = 1, 2\nconst _, two = 1, 2\n"
            ),
            ["one", "two"]
        );
    }

    #[test]
    fn pattern_wildcards_do_not_hide_real_underscore_bindings() {
        assert_eq!(
            names(Language::Python, "_ = (one, two) = value"),
            ["_", "one", "two"]
        );
        for language in [Language::JavaScript, Language::TypeScript] {
            assert_eq!(names(language, "const [_, one] = rhs;"), ["_", "one"]);
        }
    }

    #[test]
    fn decoded_literals_keep_language_identity_and_semantic_name_positions() {
        assert_eq!(
            names(Language::R, r#""\U00000041" <- function() {}"#),
            ["A"]
        );
        assert_eq!(names(Language::R, r#"'\x41' <- function() {}"#), ["A"]);
        assert_eq!(names(Language::R, r#""\303\251" <- function() {}"#), ["é"]);
        for language in [Language::JavaScript, Language::TypeScript] {
            let source = "class C { \"a\\\r\nb\"() {} \"constructor\"() {} }";
            let (symbols, defects) =
                parse_source_symbols(Path::new("fixture"), language, source).unwrap();
            assert_eq!(defects, 0);
            assert_eq!(
                symbols
                    .iter()
                    .map(|s| s.qualified_name.as_str())
                    .collect::<Vec<_>>(),
                ["C", "C::ab", "C::constructor"]
            );
            assert_eq!(symbols[2].kind, "constructor");
            let source = r#"class C { "a.b"() {} }"#;
            let (symbols, _) =
                parse_source_symbols(Path::new("fixture"), language, source).unwrap();
            assert_eq!(symbols[1].name_position, Some((0, 11)));
        }
    }

    #[test]
    fn literal_code_names_remain_atomic_canonical_segments() {
        assert_eq!(
            names(Language::R, "foo.bar <- function() { 1 }"),
            [r#"["foo.bar"]"#]
        );
        assert_eq!(
            names(Language::Hcl, r#"resource "type" "a.b" { x = 1 }"#),
            [
                r#"resource::type::["a.b"]"#,
                r#"resource::type::["a.b"]::x"#
            ]
        );
        for language in [Language::JavaScript, Language::TypeScript] {
            assert_eq!(
                names(language, r#"class C { "a.b"() {} 'a  b'() {} }"#),
                ["C", r#"C::["a.b"]"#, r#"C::["a  b"]"#]
            );
        }
        assert_eq!(
            names(Language::Cpp, "namespace A { struct B { void f(); }; }"),
            ["A", "A::B", "A::B::f"]
        );
        assert_eq!(
            names(Language::Kotlin, "val one = object { fun member() {} }"),
            ["one", "one::member"]
        );
    }
}
