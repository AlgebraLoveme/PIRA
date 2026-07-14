use std::path::{Path, PathBuf};

use tree_sitter::{InputEdit, Node, Parser, Point, Tree};

use crate::c_family_recovery::{
    aggressive_parser_view, conditional_branch_parser_views, conditional_branch_parser_views_plain,
    focused_parser_views, macro_lambda_statement_parser_view, normalized_parser_view,
};
use crate::language::Language;
use crate::language_recovery::{
    csharp_recent_syntax_parser_view, go_new_value_parser_view,
    typescript_recent_syntax_parser_view,
};
use crate::model::{ParseState, Symbol};
use crate::util::{hash16, one_line, percent_encode, read_source, source_slice};

pub struct ParsedFile {
    pub path: PathBuf,
    pub language: Language,
    pub source: String,
    pub tree: Tree,
    pub symbols: Vec<Symbol>,
    pub state: ParseState,
    pub defects: usize,
    pub original_defects: usize,
    pub raw_defects: usize,
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
    let source = read_source(path)?;
    let mut parser = language.parser(path)?;
    let original_tree = parser
        .parse(source.as_bytes(), None)
        .ok_or_else(|| format!("{} parser returned no tree", language.name()))?;
    let raw_defects = count_defects(original_tree.root_node());
    let original_defects = count_navigation_defects(original_tree.root_node(), language);
    let mut tree = original_tree.clone();
    let mut defects = original_defects;
    let mut recovered_tree = false;
    let mut branch_symbol_sets = Vec::<(usize, Vec<Symbol>)>::new();
    let force_csharp_extension_view = language == Language::CSharp
        && has_misparsed_csharp_extension_block(original_tree.root_node(), &source);
    if original_defects > 0 && matches!(language, Language::C | Language::Cpp | Language::Cuda) {
        let normalized = normalized_parser_view(&source);
        let normalized_source = normalized.as_deref().unwrap_or(&source);
        let mut normalized_defects = original_defects;
        let mut normalized_tree = original_tree.clone();
        if normalized.is_some()
            && let Some(candidate) =
                parse_aligned_view(&mut parser, &source, normalized_source, &original_tree)
        {
            normalized_defects = count_navigation_defects(candidate.root_node(), language);
            normalized_tree = candidate.clone();
            select_better_tree(
                candidate,
                normalized_defects,
                &mut tree,
                &mut defects,
                &mut recovered_tree,
            );
        }

        if normalized_defects > 0
            && let Some(aggressive) = aggressive_parser_view(&source, normalized_source)
            && let Some(candidate) = parse_aligned_view(
                &mut parser,
                normalized_source,
                &aggressive,
                &normalized_tree,
            )
        {
            let aggressive_defects = count_navigation_defects(candidate.root_node(), language);
            select_better_tree(
                candidate,
                aggressive_defects,
                &mut tree,
                &mut defects,
                &mut recovered_tree,
            );
        }

        // Combining independent masking passes can occasionally hide a
        // construct that one focused pass preserves. Focused candidates are
        // available even when no combined view was needed or generated.
        if defects > 0 {
            for focused in focused_parser_views(&source, normalized_source) {
                let Some(candidate) =
                    parse_aligned_view(&mut parser, normalized_source, &focused, &normalized_tree)
                else {
                    continue;
                };
                let candidate_defects = count_navigation_defects(candidate.root_node(), language);
                select_better_tree(
                    candidate,
                    candidate_defects,
                    &mut tree,
                    &mut defects,
                    &mut recovered_tree,
                );
            }
        }

        if defects > 0 {
            for branch in conditional_branch_parser_views(&source, normalized_source) {
                let Some(candidate) =
                    parse_aligned_view(&mut parser, normalized_source, &branch, &normalized_tree)
                else {
                    continue;
                };
                let candidate_defects = count_navigation_defects(candidate.root_node(), language);
                branch_symbol_sets.push((
                    candidate_defects,
                    collect_symbols(&candidate, language, &source),
                ));
                select_better_tree(
                    candidate,
                    candidate_defects,
                    &mut tree,
                    &mut defects,
                    &mut recovered_tree,
                );
            }
        }

        if defects > 0
            && let Some(macro_lambda) =
                macro_lambda_statement_parser_view(&source, normalized_source)
            && let Some(candidate) = parse_aligned_view(
                &mut parser,
                normalized_source,
                &macro_lambda,
                &normalized_tree,
            )
        {
            let candidate_defects = count_navigation_defects(candidate.root_node(), language);
            select_better_tree(
                candidate,
                candidate_defects,
                &mut tree,
                &mut defects,
                &mut recovered_tree,
            );
        }
    }
    let recent_view = (original_defects > 0 || force_csharp_extension_view)
        .then(|| match language {
            Language::Go => go_new_value_parser_view(&source),
            Language::TypeScript => typescript_recent_syntax_parser_view(&source),
            Language::CSharp => csharp_recent_syntax_parser_view(&source),
            _ => None,
        })
        .flatten();
    if let Some(view) = recent_view.as_deref()
        && let Some(candidate) = parse_aligned_view(&mut parser, &source, view, &original_tree)
    {
        let candidate_defects = count_navigation_defects(candidate.root_node(), language);
        if force_csharp_extension_view && candidate_defects <= defects {
            tree = candidate;
            defects = candidate_defects;
            recovered_tree = true;
        } else {
            select_better_tree(
                candidate,
                candidate_defects,
                &mut tree,
                &mut defects,
                &mut recovered_tree,
            );
        }
    }
    if defects > 0 && language == Language::CSharp {
        let base = recent_view.as_deref().unwrap_or(&source);
        for branch in conditional_branch_parser_views_plain(&source, base) {
            let Some(candidate) = parse_aligned_view(&mut parser, &source, &branch, &original_tree)
            else {
                continue;
            };
            let candidate_defects = count_navigation_defects(candidate.root_node(), language);
            branch_symbol_sets.push((
                candidate_defects,
                collect_symbols(&candidate, language, &source),
            ));
            select_better_tree(
                candidate,
                candidate_defects,
                &mut tree,
                &mut defects,
                &mut recovered_tree,
            );
        }
    }
    let state = classify_parse_state(raw_defects, defects, recovered_tree);
    let mut symbols = collect_symbols(&tree, language, &source);
    for (candidate_defects, candidate_symbols) in branch_symbol_sets {
        if candidate_defects == defects {
            symbols.extend(candidate_symbols);
        }
    }
    if recovered_tree && matches!(language, Language::C | Language::Cpp | Language::Cuda) {
        merge_original_symbols(
            &mut symbols,
            collect_symbols(&original_tree, language, &source),
        );
    }
    symbols.sort_by_key(|symbol| (symbol.start_byte, symbol.end_byte));
    symbols.dedup_by(|left, right| {
        left.start_byte == right.start_byte
            && left.end_byte == right.end_byte
            && left.kind == right.kind
            && left.qualified_name == right.qualified_name
    });
    Ok(ParsedFile {
        path: path.to_path_buf(),
        language,
        source,
        tree,
        symbols,
        state,
        defects,
        original_defects,
        raw_defects,
    })
}

fn classify_parse_state(
    raw_defects: usize,
    navigation_defects: usize,
    recovered_tree: bool,
) -> ParseState {
    if navigation_defects > 0 {
        ParseState::Partial
    } else if raw_defects == 0 && !recovered_tree {
        ParseState::Ok
    } else {
        ParseState::Recovered
    }
}

fn merge_original_symbols(recovered: &mut Vec<Symbol>, original: Vec<Symbol>) {
    for source_symbol in original {
        if is_callable_symbol(&source_symbol) && symbol_leaf_name(&source_symbol) == "namespace" {
            continue;
        }
        if recovered.iter().any(|candidate| {
            candidate.kind == source_symbol.kind
                && candidate.start_byte == source_symbol.start_byte
                && candidate.end_byte == source_symbol.end_byte
                && symbol_leaf_name(candidate) == symbol_leaf_name(&source_symbol)
        }) {
            continue;
        }
        if let Some(candidate) = recovered.iter_mut().find(|candidate| {
            is_callable_symbol(candidate)
                && is_callable_symbol(&source_symbol)
                && candidate.start_byte < source_symbol.end_byte
                && source_symbol.start_byte < candidate.end_byte
                && symbol_leaf_name(candidate) == symbol_leaf_name(&source_symbol)
        }) {
            if candidate.end_byte == source_symbol.end_byte
                && source_symbol.start_byte < candidate.start_byte
            {
                candidate.signature = source_symbol.signature;
                candidate.start_byte = source_symbol.start_byte;
                candidate.start_row = source_symbol.start_row;
                candidate.start_column = source_symbol.start_column;
            }
            continue;
        }
        if looks_like_macro_symbol(&source_symbol)
            && recovered.iter().any(|candidate| {
                is_callable_symbol(candidate)
                    && candidate.start_byte < source_symbol.end_byte
                    && source_symbol.start_byte < candidate.end_byte
            })
        {
            continue;
        }
        recovered.push(source_symbol);
    }
}

fn is_callable_symbol(symbol: &Symbol) -> bool {
    matches!(
        symbol.kind,
        "function" | "method" | "constructor" | "destructor"
    )
}

fn symbol_leaf_name(symbol: &Symbol) -> &str {
    symbol
        .qualified_name
        .rsplit([':', '.'])
        .find(|part| !part.is_empty())
        .unwrap_or(&symbol.qualified_name)
}

fn looks_like_macro_symbol(symbol: &Symbol) -> bool {
    let name = symbol_leaf_name(symbol).as_bytes();
    name.len() >= 3
        && name.iter().any(u8::is_ascii_uppercase)
        && name
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn parse_aligned_view(
    parser: &mut Parser,
    previous_source: &str,
    source: &str,
    previous_tree: &Tree,
) -> Option<Tree> {
    const MAX_INCREMENTAL_EDITS: usize = 4_096;

    if previous_source.len() != source.len() {
        return parser.parse(source.as_bytes(), None);
    }

    let previous = previous_source.as_bytes();
    let next = source.as_bytes();
    let line_starts = line_starts(previous);
    let mut ranges = Vec::new();
    let mut index = 0usize;
    while index < previous.len() {
        if previous[index] == next[index] {
            index += 1;
            continue;
        }
        if previous[index] == b'\n' || next[index] == b'\n' {
            return parser.parse(source.as_bytes(), None);
        }
        let start = index;
        index += 1;
        while index < previous.len() && previous[index] != next[index] {
            if previous[index] == b'\n' || next[index] == b'\n' {
                return parser.parse(source.as_bytes(), None);
            }
            index += 1;
        }
        ranges.push((start, index));
        if ranges.len() > MAX_INCREMENTAL_EDITS {
            return parser.parse(source.as_bytes(), None);
        }
    }
    if ranges.is_empty() {
        return Some(previous_tree.clone());
    }

    let mut edited = previous_tree.clone();
    for (start, end) in ranges.into_iter().rev() {
        edited.edit(&InputEdit {
            start_byte: start,
            old_end_byte: end,
            new_end_byte: end,
            start_position: point_at(start, &line_starts),
            old_end_position: point_at(end, &line_starts),
            new_end_position: point_at(end, &line_starts),
        });
    }
    parser.parse(source.as_bytes(), Some(&edited))
}

fn line_starts(source: &[u8]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(source.len() / 40 + 1);
    starts.push(0);
    for (index, byte) in source.iter().enumerate() {
        if *byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn point_at(byte: usize, line_starts: &[usize]) -> Point {
    let row = line_starts.partition_point(|start| *start <= byte) - 1;
    Point::new(row, byte - line_starts[row])
}

fn select_better_tree(
    candidate: Tree,
    candidate_defects: usize,
    tree: &mut Tree,
    defects: &mut usize,
    recovered_tree: &mut bool,
) {
    if candidate_defects < *defects {
        *tree = candidate;
        *defects = candidate_defects;
        *recovered_tree = true;
    }
}

fn collect_symbols(tree: &Tree, language: Language, source: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
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
    }
    symbols
}

fn push_symbol(
    node: Node<'_>,
    name_node: Node<'_>,
    source: &str,
    qualification: (Option<&str>, &str),
    kind: &'static str,
    depth: usize,
    output: &mut Vec<Symbol>,
) -> String {
    let name = one_line(&source_slice(
        source,
        name_node.start_byte(),
        name_node.end_byte(),
    ));
    push_symbol_name(node, &name, source, qualification, kind, depth, output)
}

fn push_symbol_name(
    node: Node<'_>,
    name: &str,
    source: &str,
    qualification: (Option<&str>, &str),
    kind: &'static str,
    depth: usize,
    output: &mut Vec<Symbol>,
) -> String {
    let name = one_line(name);
    let qualified = qualify(qualification.0, &name, qualification.1);
    output.push(Symbol {
        kind,
        qualified_name: qualified.clone(),
        signature: signature(node, source, node.start_byte()),
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

fn walk_java(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    output: &mut Vec<Symbol>,
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

fn walk_c_family(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    depth: usize,
    cpp: bool,
    output: &mut Vec<Symbol>,
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
        let raw_name = one_line(&source_slice(
            source,
            name_node.start_byte(),
            name_node.end_byte(),
        ));
        let final_name = raw_name.rsplit("::").next().unwrap_or(&raw_name);
        let explicit_owner = raw_name.rsplit_once("::").map(|(owner, _)| owner);
        let owner = explicit_owner.or(parent);
        let member_context = explicit_owner.is_some() || inside_class_like(node);
        let kind = if cpp && member_context {
            if owner.is_some_and(|value| value.rsplit("::").next() == Some(final_name)) {
                "constructor"
            } else {
                "method"
            }
        } else {
            "function"
        };
        push_symbol(node, name_node, source, (parent, "::"), kind, depth, output);
        return;
    }
    if matches!(node.kind(), "declaration" | "field_declaration") {
        if let Some(function) = descendant_with_kind(node, "function_declarator")
            && let Some(name_node) = declarator_name(function)
        {
            let name = source_slice(source, name_node.start_byte(), name_node.end_byte());
            let leaf = name.rsplit("::").next().unwrap_or(name.as_ref());
            let owner_leaf = parent.and_then(|value| value.rsplit("::").next());
            let member_context = name.contains("::") || inside_class_like(node);
            let kind = if cpp && member_context && owner_leaf == Some(leaf) {
                "constructor"
            } else if cpp && member_context {
                "method"
            } else {
                "function"
            };
            push_symbol(node, name_node, source, (parent, "::"), kind, depth, output);
            return;
        }
        if parent.is_some()
            && let Some(declarator) = node.child_by_field_name("declarator")
            && let Some(name_node) = declarator_name(declarator)
        {
            push_symbol(
                node,
                name_node,
                source,
                (parent, "::"),
                "field",
                depth,
                output,
            );
            return;
        }
    }
    if cpp
        && node.kind() == "function_declarator"
        && let Some(name_node) = declarator_name(node)
    {
        let name = source_slice(source, name_node.start_byte(), name_node.end_byte());
        let leaf = name.rsplit("::").next().unwrap_or(name.as_ref());
        let owner_leaf = parent.and_then(|value| value.rsplit("::").next());
        let member_context = name.contains("::") || inside_class_like(node);
        let kind = if member_context && owner_leaf == Some(leaf) {
            "constructor"
        } else if member_context {
            "method"
        } else {
            "function"
        };
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
    output: &mut Vec<Symbol>,
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
    output: &mut Vec<Symbol>,
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
    if matches!(node.kind(), "const_spec" | "var_spec")
        && parent.is_none()
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
    output: &mut Vec<Symbol>,
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
        let qualified = push_symbol(node, name_node, source, (parent, "."), kind, depth, output);
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
        let qualified = push_symbol(node, name_node, source, (parent, "."), kind, depth, output);
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
        push_symbol(
            node,
            name_node,
            source,
            (parent, "."),
            "variant",
            depth,
            output,
        );
        return;
    }
    if matches!(
        node.kind(),
        "public_field_definition" | "field_definition" | "property_signature"
    ) && parent.is_some()
        && let Some(name_node) = node.child_by_field_name("name")
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
            let kind = if function_value {
                "function"
            } else {
                "binding"
            };
            let qualified =
                push_symbol(node, name_node, source, (parent, "."), kind, depth, output);
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

fn original_line_starts_with(source: &str, byte: usize, prefix: &str) -> bool {
    let line_start = source
        .get(..byte)
        .and_then(|head| head.rfind('\n').map(|position| position + 1))
        .unwrap_or(0);
    let line_end = source
        .get(byte..)
        .and_then(|tail| tail.find('\n').map(|offset| byte + offset))
        .unwrap_or(source.len());
    source
        .get(line_start..line_end)
        .is_some_and(|line| line.trim_start().starts_with(prefix))
}

fn has_misparsed_csharp_extension_block(node: Node<'_>, source: &str) -> bool {
    if node.kind() == "constructor_declaration"
        && node.child_by_field_name("name").is_some_and(|name| {
            source_slice(source, name.start_byte(), name.end_byte()) == "extension"
        })
    {
        let mut ancestor = node.parent();
        while let Some(parent) = ancestor {
            if parent.kind() == "class_declaration" {
                return parent.child_by_field_name("name").is_some_and(|name| {
                    source_slice(source, name.start_byte(), name.end_byte()) != "extension"
                });
            }
            ancestor = parent.parent();
        }
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| has_misparsed_csharp_extension_block(child, source))
}

fn walk_csharp_root(node: Node<'_>, source: &str, output: &mut Vec<Symbol>) {
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
    output: &mut Vec<Symbol>,
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
        if node.kind() == "class_declaration"
            && original_line_starts_with(source, node.start_byte(), "extension")
        {
            if let Some(body) = node.child_by_field_name("body") {
                walk_named_children(body, |child| {
                    walk_csharp(child, source, parent, depth, output)
                });
            }
            return;
        }
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
            &name,
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
            &name,
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
    output: &mut Vec<Symbol>,
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

fn walk_php_root(root: Node<'_>, source: &str, output: &mut Vec<Symbol>) {
    let mut namespace = None;
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "namespace_definition" {
            walk_php(child, source, namespace.as_deref(), 0, output);
            continue;
        }
        let Some(name) = child.child_by_field_name("name") else {
            continue;
        };
        let namespace_name = one_line(&source_slice(source, name.start_byte(), name.end_byte()));
        push_symbol(child, name, source, (None, "\\"), "namespace", 0, output);
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
    output: &mut Vec<Symbol>,
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
    output: &mut Vec<Symbol>,
) {
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
                .and_then(|child| descendant_with_kind(child, "simple_identifier"))
                .or_else(|| named_child_with_kind(&node, &["simple_identifier"])),
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
    output: &mut Vec<Symbol>,
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
    output: &mut Vec<Symbol>,
) {
    if node.kind() == "block" {
        let mut cursor = node.walk();
        let segments = node
            .named_children(&mut cursor)
            .filter(|child| matches!(child.kind(), "identifier" | "string_lit"))
            .map(|child| {
                source_slice(source, child.start_byte(), child.end_byte())
                    .trim_matches(['\'', '"'])
                    .to_owned()
            })
            .collect::<Vec<_>>();
        if !segments.is_empty() {
            let name = segments.join(".");
            let qualified =
                push_symbol_name(node, &name, source, (parent, "."), "block", depth, output);
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
    output: &mut Vec<Symbol>,
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
            let qualified =
                push_symbol(node, name, source, (parent, "."), "function", depth, output);
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
    if matches!(
        node.kind(),
        "identifier" | "field_identifier" | "qualified_identifier"
    ) {
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

fn inside_class_like(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(
            parent.kind(),
            "class_specifier" | "struct_specifier" | "union_specifier"
        ) {
            return true;
        }
        current = parent.parent();
    }
    false
}

fn count_defects(node: Node<'_>) -> usize {
    let mut count = usize::from(node.is_error() || node.is_missing());
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count += count_defects(child);
    }
    count
}

fn count_navigation_defects(node: Node<'_>, language: Language) -> usize {
    let mut count = usize::from(node.is_error() || node.is_missing());
    let outlined_c_family_callable =
        matches!(language, Language::C | Language::Cpp | Language::Cuda)
            && node.kind() == "function_definition"
            && node
                .child_by_field_name("declarator")
                .and_then(declarator_name)
                .is_some();
    let outlined_csharp_member = language == Language::CSharp
        && matches!(
            node.kind(),
            "method_declaration"
                | "constructor_declaration"
                | "property_declaration"
                | "operator_declaration"
                | "conversion_operator_declaration"
        )
        && (node.child_by_field_name("name").is_some()
            || node.child_by_field_name("operator").is_some()
            || node.child_by_field_name("type").is_some());
    let body = (outlined_c_family_callable || outlined_csharp_member)
        .then(|| node.child_by_field_name("body"))
        .flatten();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if body == Some(child) {
            continue;
        }
        count += count_navigation_defects(child, language);
    }
    count
}

fn walk_python(
    node: Node<'_>,
    source: &str,
    parent: Option<&str>,
    parent_is_class: bool,
    depth: usize,
    output: &mut Vec<Symbol>,
) {
    if parent.is_none() && node.kind() == "assignment" {
        if let Some(left) = node.child_by_field_name("left") {
            add_python_bindings(node, left, source, depth, output);
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
                Some(node.start_byte()),
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
    output: &mut Vec<Symbol>,
) {
    if target.kind() == "identifier" {
        push_symbol(
            assignment,
            target,
            source,
            (None, "."),
            "binding",
            depth,
            output,
        );
        return;
    }
    if matches!(target.kind(), "pattern_list" | "tuple" | "list") {
        let mut cursor = target.walk();
        for child in target.named_children(&mut cursor) {
            add_python_bindings(assignment, child, source, depth, output);
        }
    }
}

fn add_python_definition(
    node: Node<'_>,
    range_start: Option<usize>,
    source: &str,
    parent: Option<&str>,
    parent_is_class: bool,
    depth: usize,
    output: &mut Vec<Symbol>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = source_slice(source, name_node.start_byte(), name_node.end_byte()).into_owned();
    let qualified = qualify(parent, &name, ".");
    let kind = if node.kind() == "class_definition" {
        "class"
    } else if parent_is_class {
        "method"
    } else {
        "function"
    };
    let start_byte = range_start.unwrap_or(node.start_byte());
    let (start_row, start_column) = if start_byte == node.start_byte() {
        let point = node.start_position();
        (point.row, point.column)
    } else {
        byte_to_point(source, start_byte)
    };
    output.push(Symbol {
        kind,
        qualified_name: qualified.clone(),
        signature: signature(node, source, start_byte),
        start_byte,
        end_byte: node.end_byte(),
        start_row,
        start_column,
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
    output: &mut Vec<Symbol>,
) {
    if node.kind() == "impl_item" {
        let type_name = node
            .child_by_field_name("type")
            .map(|part| one_line(&source_slice(source, part.start_byte(), part.end_byte())))
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
        "function_item" if parent.is_some() => Some("method"),
        "function_item" => Some("function"),
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
            let qualified = qualify(parent, &name, "::");
            let start_byte = rust_attached_start(node, source);
            let (start_row, start_column) = byte_to_point(source, start_byte);
            output.push(Symbol {
                kind,
                qualified_name: qualified.clone(),
                // Attached docs/attributes belong to `show`, not the compact signature.
                signature: signature(node, source, node.start_byte()),
                start_byte,
                end_byte: node.end_byte(),
                start_row,
                start_column,
                end_row: node.end_position().row,
                end_column: node.end_position().column,
                depth,
            });
            if !matches!(node.kind(), "function_item") {
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

fn rust_attached_start(node: Node<'_>, source: &str) -> usize {
    let mut start = node.start_byte();
    let mut previous = node.prev_named_sibling();
    while let Some(candidate) = previous {
        let text = source_slice(source, candidate.start_byte(), candidate.end_byte());
        let attachable = candidate.kind() == "attribute_item"
            || (candidate.kind() == "line_comment" && text.trim_start().starts_with("///"))
            || (candidate.kind() == "block_comment" && text.trim_start().starts_with("/**"));
        if !attachable {
            break;
        }
        let gap = source_slice(source, candidate.end_byte(), start);
        if gap.bytes().filter(|byte| *byte == b'\n').count() > 1 {
            break;
        }
        start = candidate.start_byte();
        previous = candidate.prev_named_sibling();
    }
    start
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
    match parent {
        Some(parent) if !parent.is_empty() => format!("{parent}{separator}{name}"),
        _ => name.to_owned(),
    }
}

fn byte_to_point(source: &str, byte: usize) -> (usize, usize) {
    let prefix = source.get(..byte).unwrap_or_default();
    let row = prefix.bytes().filter(|value| *value == b'\n').count();
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix.len(), |(_, tail)| tail.len());
    (row, column)
}

#[cfg(test)]
mod parse_completeness_tests {
    use super::{
        classify_parse_state, count_defects, count_navigation_defects, merge_original_symbols,
    };
    use crate::language::Language;
    use crate::model::{ParseState, Symbol};
    use tree_sitter::Parser;

    fn symbol(name: &str, start: usize, end: usize) -> Symbol {
        Symbol {
            kind: "function",
            qualified_name: name.into(),
            signature: name.into(),
            start_byte: start,
            end_byte: end,
            start_row: 0,
            start_column: start,
            end_row: 0,
            end_column: end,
            depth: 0,
        }
    }

    #[test]
    fn c_family_function_body_errors_do_not_mark_navigation_partial() {
        let source = "__global__ void run(int n) {\n\
  for (auto i = decltype(n){0}; i < n; ++i) {}\n\
}\n";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cuda::LANGUAGE.into())
            .expect("cuda grammar");
        let tree = parser.parse(source.as_bytes(), None).expect("tree");
        assert!(count_defects(tree.root_node()) > 0);
        assert_eq!(
            count_navigation_defects(tree.root_node(), Language::Cuda),
            0
        );
    }

    #[test]
    fn parse_state_never_calls_remaining_navigation_defects_recovered() {
        assert_eq!(classify_parse_state(5, 1, true), ParseState::Partial);
        assert_eq!(classify_parse_state(0, 0, false), ParseState::Ok);
        assert_eq!(classify_parse_state(2, 0, false), ParseState::Recovered);
        assert_eq!(classify_parse_state(0, 0, true), ParseState::Recovered);
    }

    #[test]
    fn recovery_symbol_merge_keeps_conditional_alternatives_and_drops_cascades() {
        let mut recovered = vec![symbol("umain", 0, 100), symbol("actual", 120, 220)];
        merge_original_symbols(
            &mut recovered,
            vec![
                symbol("main", 10, 100),
                symbol("SMALL_INDEX", 140, 400),
                symbol("actual", 125, 130),
                symbol("namespace", 230, 260),
            ],
        );
        let names = recovered
            .iter()
            .map(|item| item.qualified_name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"umain"));
        assert!(names.contains(&"main"));
        assert_eq!(names.iter().filter(|name| **name == "actual").count(), 1);
        assert!(!names.contains(&"SMALL_INDEX"));
        assert!(!names.contains(&"namespace"));
    }
}
