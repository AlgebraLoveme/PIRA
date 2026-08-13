use std::fs;
use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::language::Language;
use crate::model::ImportEdge;
use crate::parse::ParsedSyntax;
use crate::util::{absolute_lexical, display_path, one_line, source_slice};

pub fn imports(parsed: &ParsedSyntax, cwd: &Path) -> Vec<ImportEdge> {
    let mut output = Vec::new();
    match parsed.language {
        Language::Python => collect_python(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Rust => {
            let source_root = find_rust_source_root(&parsed.path, cwd);
            collect_rust(
                parsed.tree.root_node(),
                parsed,
                cwd,
                &source_root,
                &mut output,
            )
        }
        Language::Java => collect_java(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::C | Language::Cpp | Language::Cuda => {
            collect_c_family(parsed.tree.root_node(), parsed, cwd, &mut output)
        }
        Language::Bash => collect_bash(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Go => collect_go(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::JavaScript | Language::TypeScript => {
            collect_ecmascript(parsed.tree.root_node(), parsed, cwd, &mut output)
        }
        Language::CSharp => collect_csharp(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::PowerShell => {
            collect_powershell(parsed.tree.root_node(), parsed, cwd, &mut output)
        }
        Language::Php => collect_php(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Kotlin => collect_kotlin(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Lua => collect_lua(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Hcl => collect_hcl(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::R => collect_r(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Ruby => collect_ruby(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Swift => collect_swift(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Scala => collect_scala(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Dart => collect_dart(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Elixir => collect_elixir(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Julia => collect_julia(parsed.tree.root_node(), parsed, cwd, &mut output),
        Language::Json | Language::Jsonc | Language::Yaml | Language::Toml | Language::Markdown => {
        }
    }
    output.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.text.cmp(&right.text))
    });
    output
}

fn collect_java(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "import_declaration" {
        let text = node_text(node, parsed);
        let module = text
            .trim_start_matches("import ")
            .trim_start_matches("static ")
            .trim_end_matches(';')
            .trim_end_matches(".*");
        let mut relative = PathBuf::new();
        for part in module.split('.') {
            relative.push(part);
        }
        relative.set_extension("java");
        let targets = ancestor_source_targets(
            &parsed.path,
            cwd,
            &relative,
            &["", "src", "src/main/java", "src/test/java"],
        );
        let (target, label, resolution) =
            resolved_or_ambiguous(targets, module.split('.').next().unwrap_or(module), cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    recurse(node, |child| collect_java(child, parsed, cwd, output));
}

fn collect_c_family(
    node: Node<'_>,
    parsed: &ParsedSyntax,
    cwd: &Path,
    output: &mut Vec<ImportEdge>,
) {
    if node.kind() == "preproc_include" {
        let text = node_text(node, parsed);
        let include = text
            .split_once("include")
            .map(|(_, rest)| rest.trim())
            .unwrap_or_default();
        let quoted = include.starts_with('"') && include.ends_with('"');
        let name = include.trim_matches(['"', '<', '>']);
        let target = quoted.then(|| parsed.path.parent().unwrap_or(cwd).join(name));
        let (target, label, resolution) =
            resolved_or_external(target.filter(|candidate| candidate.is_file()), name, cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    recurse(node, |child| collect_c_family(child, parsed, cwd, output));
}

fn collect_bash(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "command" {
        let text = node_text(node, parsed);
        if text.starts_with("source ") || text.starts_with(". ") {
            let argument = text
                .split_once(' ')
                .map(|(_, value)| value)
                .unwrap_or_default();
            let clean = argument.trim_matches(['\'', '"']);
            let name = clean
                .rsplit('/')
                .next()
                .unwrap_or(clean)
                .trim_matches(['\'', '"']);
            let candidate = parsed.path.parent().unwrap_or(cwd).join(name);
            let (target, label, resolution) =
                resolved_or_external(candidate.is_file().then_some(candidate), name, cwd);
            output.push(edge(parsed, node, text, target, label, resolution));
            return;
        }
    }
    recurse(node, |child| collect_bash(child, parsed, cwd, output));
}

fn collect_go(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "import_spec"
        && let Some(path_node) = node.child_by_field_name("path")
    {
        let text = node_text(node, parsed);
        let module = unquote(&source_slice(
            &parsed.source,
            path_node.start_byte(),
            path_node.end_byte(),
        ));
        let (target, label, resolution) = resolved_or_external(None, &module, cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    recurse(node, |child| collect_go(child, parsed, cwd, output));
}

fn collect_ecmascript(
    node: Node<'_>,
    parsed: &ParsedSyntax,
    cwd: &Path,
    output: &mut Vec<ImportEdge>,
) {
    if matches!(node.kind(), "import_statement" | "export_statement")
        && let Some(source_node) = node.child_by_field_name("source")
    {
        let text = node_text(node, parsed);
        let module = unquote(&source_slice(
            &parsed.source,
            source_node.start_byte(),
            source_node.end_byte(),
        ));
        let (target, label, resolution) = resolve_ecmascript(&parsed.path, &module, cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    if node.kind() == "call_expression"
        && node
            .child_by_field_name("function")
            .is_some_and(|function| node_text(function, parsed) == "require")
        && let Some(arguments) = node.child_by_field_name("arguments")
        && let Some(source_node) = first_named_kind(arguments, "string")
    {
        let text = node_text(node, parsed);
        let module = unquote(&source_slice(
            &parsed.source,
            source_node.start_byte(),
            source_node.end_byte(),
        ));
        let (target, label, resolution) = resolve_ecmascript(&parsed.path, &module, cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    recurse(node, |child| collect_ecmascript(child, parsed, cwd, output));
}

fn collect_csharp(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "using_directive" {
        let text = node_text(node, parsed);
        let module = text
            .trim_start_matches("global ")
            .trim_start_matches("using ")
            .trim_end_matches(';')
            .trim();
        let (target, label, resolution) = resolved_or_external(None, module, cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    recurse(node, |child| collect_csharp(child, parsed, cwd, output));
}

fn collect_powershell(
    node: Node<'_>,
    parsed: &ParsedSyntax,
    cwd: &Path,
    output: &mut Vec<ImportEdge>,
) {
    if node.kind() == "command" {
        let text = node_text(node, parsed);
        let lower = text.to_ascii_lowercase();
        let argument = if lower.starts_with("import-module ") {
            text.split_once(char::is_whitespace)
                .map(|(_, rest)| rest.trim())
        } else if text.starts_with(". ") {
            text.split_once(' ').map(|(_, rest)| rest.trim())
        } else {
            None
        };
        if let Some(argument) = argument {
            let module = unquote(
                argument
                    .split_whitespace()
                    .next()
                    .unwrap_or(argument)
                    .trim_matches(['\'', '"']),
            )
            .replace('\\', "/");
            let candidate = parsed.path.parent().unwrap_or(cwd).join(&module);
            let (target, label, resolution) = resolved_or_external(
                candidate.is_file().then_some(candidate),
                module.trim_start_matches("./"),
                cwd,
            );
            output.push(edge(parsed, node, text, target, label, resolution));
            return;
        }
    }
    recurse(node, |child| collect_powershell(child, parsed, cwd, output));
}

fn collect_php(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "namespace_use_declaration" {
        let text = node_text(node, parsed);
        let module = text.trim_start_matches("use ").trim_end_matches(';').trim();
        let (target, label, resolution) = resolved_or_external(None, module, cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    if matches!(
        node.kind(),
        "include_expression"
            | "include_once_expression"
            | "require_expression"
            | "require_once_expression"
    ) {
        let text = node_text(node, parsed);
        if let Some(string) = descendant_kind(node, "string") {
            let path = unquote(&source_slice(
                &parsed.source,
                string.start_byte(),
                string.end_byte(),
            ));
            let candidate = parsed.path.parent().unwrap_or(cwd).join(&path);
            let (target, label, resolution) =
                resolved_or_external(candidate.is_file().then_some(candidate), &path, cwd);
            output.push(edge(parsed, node, text, target, label, resolution));
            return;
        }
    }
    recurse(node, |child| collect_php(child, parsed, cwd, output));
}

fn collect_kotlin(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "import_header" {
        let text = node_text(node, parsed);
        let module = text
            .trim_start_matches("import ")
            .split_once(" as ")
            .map_or(text.trim_start_matches("import "), |(path, _)| path)
            .trim_end_matches(".*")
            .trim();
        let relative = PathBuf::from(module.replace('.', "/")).with_extension("kt");
        let targets = ancestor_source_targets(
            &parsed.path,
            cwd,
            &relative,
            &["", "src", "src/main/kotlin", "src/test/kotlin"],
        );
        let (target, label, resolution) =
            resolved_or_ambiguous(targets, module.split('.').next().unwrap_or(module), cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    recurse(node, |child| collect_kotlin(child, parsed, cwd, output));
}

fn collect_lua(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "function_call"
        && node
            .child_by_field_name("name")
            .is_some_and(|name| node_text(name, parsed) == "require")
        && let Some(arguments) = node.child_by_field_name("arguments")
        && let Some(string) = descendant_kind(arguments, "string")
    {
        let text = node_text(node, parsed);
        let module = unquote(&source_slice(
            &parsed.source,
            string.start_byte(),
            string.end_byte(),
        ));
        let relative = PathBuf::from(module.replace('.', "/"));
        let roots = [parsed.path.parent().unwrap_or(cwd), cwd];
        let mut candidates = roots
            .into_iter()
            .flat_map(|root| {
                [
                    root.join(&relative).with_extension("lua"),
                    root.join(&relative).join("init.lua"),
                ]
            })
            .filter(|candidate| candidate.is_file())
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        let result = if candidates.len() == 1 {
            checked_structural_target(candidates.remove(0), cwd)
        } else if candidates.len() > 1 {
            (None, format!("ambiguous:{module}"), "ambiguous")
        } else {
            resolved_or_external(None, &module, cwd)
        };
        output.push(edge(parsed, node, text, result.0, result.1, result.2));
        return;
    }
    recurse(node, |child| collect_lua(child, parsed, cwd, output));
}

fn collect_hcl(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "block" {
        let first_identifier = first_named_kind(node, "identifier");
        if first_identifier.is_some_and(|identifier| node_text(identifier, parsed) == "module")
            && let Some(body) = first_named_kind(node, "body")
        {
            let mut cursor = body.walk();
            for attribute in body
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "attribute")
            {
                if first_named_kind(attribute, "identifier")
                    .is_none_or(|identifier| node_text(identifier, parsed) != "source")
                {
                    continue;
                }
                let text = node_text(attribute, parsed);
                let Some(string) = descendant_kind(attribute, "string_lit") else {
                    continue;
                };
                let source_path = unquote(&source_slice(
                    &parsed.source,
                    string.start_byte(),
                    string.end_byte(),
                ));
                let candidate = parsed.path.parent().unwrap_or(cwd).join(&source_path);
                let result = if candidate.is_file() {
                    checked_structural_target(candidate, cwd)
                } else {
                    (None, source_path.clone(), "unresolved")
                };
                output.push(edge(parsed, attribute, text, result.0, result.1, result.2));
            }
        }
    }
    recurse(node, |child| collect_hcl(child, parsed, cwd, output));
}

fn collect_r(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "call"
        && let Some(function) = node.child_by_field_name("function")
    {
        let function_name = node_text(function, parsed);
        if matches!(
            function_name.as_str(),
            "source" | "sys.source" | "library" | "require"
        ) && let Some(arguments) = node.child_by_field_name("arguments")
        {
            let text = node_text(node, parsed);
            let literal = descendant_kind(arguments, "string")
                .or_else(|| descendant_kind(arguments, "identifier"));
            if let Some(literal) = literal {
                let value = unquote(&source_slice(
                    &parsed.source,
                    literal.start_byte(),
                    literal.end_byte(),
                ));
                let result = if matches!(function_name.as_str(), "source" | "sys.source") {
                    let candidate = parsed.path.parent().unwrap_or(cwd).join(&value);
                    resolved_or_external(candidate.is_file().then_some(candidate), &value, cwd)
                } else {
                    resolved_or_external(None, &value, cwd)
                };
                output.push(edge(parsed, node, text, result.0, result.1, result.2));
                return;
            }
        }
    }
    recurse(node, |child| collect_r(child, parsed, cwd, output));
}

fn collect_ruby(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "call"
        && let Some(method) = node.child_by_field_name("method")
    {
        let method = node_text(method, parsed);
        if matches!(method.as_str(), "require" | "require_relative" | "load")
            && let Some(arguments) = first_named_kind(node, "argument_list")
            && let Some(string) = descendant_kind(arguments, "string")
        {
            let module = unquote(&node_text(string, parsed));
            let target = if method == "require_relative" {
                resolve_relative_source(&parsed.path, &module, "rb")
            } else {
                None
            };
            let (target, label, resolution) = resolved_or_external(target, &module, cwd);
            output.push(edge(
                parsed,
                node,
                node_text(node, parsed),
                target,
                label,
                resolution,
            ));
            return;
        }
    }
    recurse(node, |child| collect_ruby(child, parsed, cwd, output));
}

fn collect_swift(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "import_declaration" {
        let text = node_text(node, parsed);
        let module = text.strip_prefix("import ").unwrap_or(&text).trim();
        let (target, label, resolution) = resolved_or_external(None, module, cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    recurse(node, |child| collect_swift(child, parsed, cwd, output));
}

fn collect_scala(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "import_declaration" {
        let text = node_text(node, parsed);
        let module = text.strip_prefix("import ").unwrap_or(&text).trim();
        let (target, label, resolution) = resolved_or_external(None, module, cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    recurse(node, |child| collect_scala(child, parsed, cwd, output));
}

fn collect_dart(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if matches!(
        node.kind(),
        "library_import" | "library_export" | "part_directive"
    ) && let Some(string) = descendant_kind(node, "string_literal")
    {
        let module = unquote(&node_text(string, parsed));
        let target = if module.starts_with("dart:") || module.starts_with("package:") {
            None
        } else {
            resolve_relative_source(&parsed.path, &module, "dart")
        };
        let (target, label, resolution) = resolved_or_external(target, &module, cwd);
        output.push(edge(
            parsed,
            node,
            node_text(node, parsed),
            target,
            label,
            resolution,
        ));
        return;
    }
    recurse(node, |child| collect_dart(child, parsed, cwd, output));
}

fn collect_elixir(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if node.kind() == "call"
        && let Some(target_node) = node.child_by_field_name("target")
    {
        let target_name = node_text(target_node, parsed);
        if matches!(target_name.as_str(), "alias" | "import" | "require" | "use")
            && let Some(arguments) = first_named_kind(node, "arguments")
            && let Some(module) = arguments.named_child(0)
        {
            let module = node_text(module, parsed);
            let (target, label, resolution) = resolved_or_external(None, &module, cwd);
            output.push(edge(
                parsed,
                node,
                node_text(node, parsed),
                target,
                label,
                resolution,
            ));
            return;
        }
    }
    recurse(node, |child| collect_elixir(child, parsed, cwd, output));
}

fn collect_julia(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if matches!(node.kind(), "using_statement" | "import_statement") {
        let text = node_text(node, parsed);
        let module = text
            .split_once(char::is_whitespace)
            .map_or(text.as_str(), |(_, module)| module)
            .trim();
        let (target, label, resolution) = resolved_or_external(None, module, cwd);
        output.push(edge(parsed, node, text, target, label, resolution));
        return;
    }
    if node.kind() == "call_expression"
        && let Some(function) = node.named_child(0)
        && node_text(function, parsed) == "include"
        && let Some(string) = descendant_kind(node, "string_literal")
    {
        let module = unquote(&node_text(string, parsed));
        let target = resolve_relative_source(&parsed.path, &module, "jl");
        let (target, label, resolution) = resolved_or_external(target, &module, cwd);
        output.push(edge(
            parsed,
            node,
            node_text(node, parsed),
            target,
            label,
            resolution,
        ));
        return;
    }
    recurse(node, |child| collect_julia(child, parsed, cwd, output));
}

fn resolve_relative_source(path: &Path, module: &str, default_extension: &str) -> Option<PathBuf> {
    let base = path.parent()?.join(module);
    if base.is_file() {
        return Some(base);
    }
    if base.extension().is_none() {
        let with_extension = base.with_extension(default_extension);
        if with_extension.is_file() {
            return Some(with_extension);
        }
    }
    None
}

fn resolve_ecmascript(
    path: &Path,
    module: &str,
    cwd: &Path,
) -> (Option<PathBuf>, String, &'static str) {
    if !module.starts_with('.') {
        let package = if module.starts_with('@') {
            module.split('/').take(2).collect::<Vec<_>>().join("/")
        } else {
            module.split('/').next().unwrap_or(module).to_owned()
        };
        return (None, format!("external:{package}"), "external");
    }
    let base = path.parent().unwrap_or(cwd).join(module);
    let extensions = ["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts"];
    let target = if base.is_file() {
        Some(base)
    } else {
        extensions
            .iter()
            .map(|extension| base.with_extension(extension))
            .find(|candidate| candidate.is_file())
            .or_else(|| {
                extensions
                    .iter()
                    .map(|extension| base.join(format!("index.{extension}")))
                    .find(|candidate| candidate.is_file())
            })
    };
    resolved_or_external(target, module, cwd)
}

fn unquote(value: &str) -> String {
    value.trim().trim_matches(['\'', '"', '`']).to_owned()
}

fn first_named_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn descendant_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
        if let Some(found) = descendant_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

fn node_text(node: Node<'_>, parsed: &ParsedSyntax) -> String {
    one_line(&source_slice(
        &parsed.source,
        node.start_byte(),
        node.end_byte(),
    ))
}

fn edge(
    parsed: &ParsedSyntax,
    node: Node<'_>,
    text: String,
    target: Option<PathBuf>,
    target_label: String,
    resolution: &'static str,
) -> ImportEdge {
    ImportEdge {
        source: parsed.path.clone(),
        line: node.start_position().row + 1,
        text,
        target,
        target_label,
        resolution,
    }
}

fn resolved_or_external(
    target: Option<PathBuf>,
    external: &str,
    cwd: &Path,
) -> (Option<PathBuf>, String, &'static str) {
    if let Some(target) = target {
        return checked_structural_target(target, cwd);
    }
    (None, format!("external:{external}"), "external")
}

fn resolved_or_ambiguous(
    mut targets: Vec<PathBuf>,
    external: &str,
    cwd: &Path,
) -> (Option<PathBuf>, String, &'static str) {
    match targets.len() {
        0 => (None, format!("external:{external}"), "external"),
        1 => checked_structural_target(targets.remove(0), cwd),
        _ => (None, format!("ambiguous:{external}"), "ambiguous"),
    }
}

fn ancestor_source_targets(
    path: &Path,
    cwd: &Path,
    relative: &Path,
    prefixes: &[&str],
) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    let mut current = path.parent();
    while let Some(root) = current {
        if !root.starts_with(cwd) {
            break;
        }
        for prefix in prefixes {
            let candidate = root.join(prefix).join(relative);
            if candidate.is_file() && !targets.contains(&candidate) {
                targets.push(candidate);
            }
        }
        if root == cwd {
            break;
        }
        current = root.parent();
    }
    targets
}

fn recurse(node: Node<'_>, mut visit: impl FnMut(Node<'_>)) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child);
    }
}

fn collect_python(node: Node<'_>, parsed: &ParsedSyntax, cwd: &Path, output: &mut Vec<ImportEdge>) {
    if matches!(node.kind(), "import_statement" | "import_from_statement") {
        let text = one_line(&source_slice(
            &parsed.source,
            node.start_byte(),
            node.end_byte(),
        ));
        let modules = if let Some(rest) = text.strip_prefix("from ") {
            rest.split_once(" import ")
                .map(|(module, _)| vec![module])
                .unwrap_or_default()
        } else {
            text.strip_prefix("import ")
                .map(|rest| {
                    rest.split(',')
                        .filter_map(|part| part.split_whitespace().next())
                        .collect()
                })
                .unwrap_or_default()
        };
        for module in modules {
            let (target, label, resolution) = resolve_python(&parsed.path, module, cwd);
            output.push(ImportEdge {
                source: parsed.path.clone(),
                line: node.start_position().row + 1,
                text: text.clone(),
                target,
                target_label: label,
                resolution,
            });
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_python(child, parsed, cwd, output);
    }
}

fn resolve_python(
    path: &Path,
    module: &str,
    cwd: &Path,
) -> (Option<PathBuf>, String, &'static str) {
    let relative_count = module.bytes().take_while(|byte| *byte == b'.').count();
    let module_name = &module[relative_count..];
    let module_path = module_name.replace('.', "/");
    let mut bases = Vec::new();
    if relative_count == 0 {
        let mut current = path.parent();
        while let Some(base) = current {
            if !base.starts_with(cwd) {
                break;
            }
            bases.push(base.to_path_buf());
            bases.push(base.join("src"));
            if base == cwd {
                break;
            }
            current = base.parent();
        }
    } else {
        let mut base = path.parent().unwrap_or(cwd).to_path_buf();
        if relative_count > 1 {
            for _ in 1..relative_count {
                base.pop();
            }
        }
        bases.push(base);
    }
    let mut targets = Vec::new();
    for mut base in bases {
        if !module_path.is_empty() {
            base.push(&module_path);
        }
        if let Some(target) = existing_module(&base, "py")
            && !targets.contains(&target)
        {
            targets.push(target);
        }
    }
    if targets.len() == 1 {
        return checked_structural_target(targets.remove(0), cwd);
    }
    let external = module_name
        .split('.')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(module)
        .trim_start_matches('.');
    if targets.len() > 1 {
        return (None, format!("ambiguous:{external}"), "ambiguous");
    }
    (None, format!("external:{external}"), "external")
}

fn existing_module(base: &Path, extension: &str) -> Option<PathBuf> {
    let file = base.with_extension(extension);
    if file.is_file() {
        return Some(file);
    }
    let package = base.join(format!("__init__.{extension}"));
    package.is_file().then_some(package)
}

fn collect_rust(
    node: Node<'_>,
    parsed: &ParsedSyntax,
    cwd: &Path,
    source_root: &Path,
    output: &mut Vec<ImportEdge>,
) {
    if node.kind() == "mod_item" && node.child_by_field_name("body").is_none() {
        let text = one_line(&source_slice(
            &parsed.source,
            node.start_byte(),
            node.end_byte(),
        ));
        if let Some(name_node) = node.child_by_field_name("name") {
            let name = source_slice(&parsed.source, name_node.start_byte(), name_node.end_byte());
            let base = parsed.path.parent().unwrap_or(cwd).join(name.as_ref());
            let target = existing_rust_module(&base).map(|path| absolute_lexical(&path, cwd));
            let resolution = if target.is_some() {
                "structural"
            } else {
                "unresolved"
            };
            let label = target
                .as_deref()
                .map(|path| display_path(path, cwd))
                .unwrap_or_else(|| format!("module:{name}"));
            output.push(ImportEdge {
                source: parsed.path.clone(),
                line: node.start_position().row + 1,
                text,
                target,
                target_label: label,
                resolution,
            });
        }
        return;
    }
    if node.kind() == "use_declaration" {
        let text = one_line(&source_slice(
            &parsed.source,
            node.start_byte(),
            node.end_byte(),
        ));
        let path_text = text
            .trim_start_matches("pub ")
            .trim_start_matches("use ")
            .trim_end_matches(';');
        let (target, label, resolution) = resolve_rust(&parsed.path, path_text, cwd, source_root);
        output.push(ImportEdge {
            source: parsed.path.clone(),
            line: node.start_position().row + 1,
            text,
            target,
            target_label: label,
            resolution,
        });
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_rust(child, parsed, cwd, source_root, output);
    }
}

fn existing_rust_module(base: &Path) -> Option<PathBuf> {
    let file = base.with_extension("rs");
    if file.is_file() {
        return Some(file);
    }
    let module = base.join("mod.rs");
    module.is_file().then_some(module)
}

fn resolve_rust(
    path: &Path,
    raw: &str,
    cwd: &Path,
    source_root: &Path,
) -> (Option<PathBuf>, String, &'static str) {
    let normalized = raw.replace(' ', "");
    let prefix = normalized
        .split(['{', '*'])
        .next()
        .unwrap_or_default()
        .trim_end_matches("::");
    let mut parts = prefix.split("::").filter(|part| !part.is_empty());
    let first = parts.next().unwrap_or_default();
    if matches!(first, "std" | "core" | "alloc") {
        return (None, format!("external:{first}"), "external");
    }
    let mut base;
    let mut segments = Vec::new();
    match first {
        "crate" => {
            base = source_root.to_path_buf();
            segments.extend(parts);
        }
        "self" => {
            base = path.parent().unwrap_or(cwd).to_path_buf();
            segments.extend(parts);
        }
        "super" => {
            base = path.parent().unwrap_or(cwd).to_path_buf();
            let mut remaining = parts.peekable();
            while remaining.peek() == Some(&"super") {
                remaining.next();
                let Some(parent) = base.parent() else {
                    return (None, "outside-workspace".into(), "blocked");
                };
                if !parent.starts_with(source_root) {
                    return (None, "outside-workspace".into(), "blocked");
                }
                base = parent.to_path_buf();
            }
            segments.extend(remaining);
        }
        "" => return (None, "external:unknown".into(), "unresolved"),
        local => {
            base = path.parent().unwrap_or(cwd).to_path_buf();
            segments.push(local);
            segments.extend(parts);
        }
    }
    if segments.is_empty() {
        return (None, format!("external:{first}"), "unresolved");
    }
    // The longest prefix that names a file/module wins; trailing segments are imported items.
    while !segments.is_empty() {
        let mut candidate = base.clone();
        for segment in &segments {
            candidate.push(segment);
        }
        if let Some(target) = existing_rust_module(&candidate) {
            return checked_structural_target(target, cwd);
        }
        segments.pop();
    }
    (None, format!("external:{first}"), "external")
}

fn checked_structural_target(
    target: PathBuf,
    cwd: &Path,
) -> (Option<PathBuf>, String, &'static str) {
    let lexical = absolute_lexical(&target, cwd);
    let canonical_root = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let canonical_target = fs::canonicalize(&lexical).unwrap_or_else(|_| lexical.clone());
    if !canonical_target.starts_with(&canonical_root) {
        return (None, "outside-workspace".into(), "blocked");
    }
    let label = display_path(&lexical, cwd);
    (Some(lexical), label, "structural")
}

fn find_rust_source_root(path: &Path, cwd: &Path) -> PathBuf {
    let mut current = path.parent().unwrap_or(cwd);
    loop {
        if current.join("lib.rs").is_file() || current.join("main.rs").is_file() {
            return current.to_path_buf();
        }
        if current == cwd {
            return cwd.to_path_buf();
        }
        let Some(parent) = current.parent() else {
            return cwd.to_path_buf();
        };
        current = parent;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::resolve_rust;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn resolves_multiple_leading_rust_super_components() {
        let root = std::env::temp_dir().join(format!(
            "pira-nav-rust-super-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("lib.rs"), "mod c; mod a;").unwrap();
        fs::write(root.join("c.rs"), "pub struct Thing;").unwrap();
        let source = root.join("a/b.rs");
        fs::write(&source, "use super::super::c::Thing;").unwrap();

        let (target, _, resolution) = resolve_rust(&source, "super::super::c::Thing", &root, &root);
        assert_eq!(resolution, "structural");
        assert_eq!(target, Some(root.join("c.rs")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn blocks_rust_super_components_above_source_root() {
        let root = PathBuf::from("/workspace/src");
        let source = root.join("a.rs");
        let (target, label, resolution) =
            resolve_rust(&source, "super::super::outside", &root, &root);
        assert!(target.is_none());
        assert_eq!(label, "outside-workspace");
        assert_eq!(resolution, "blocked");
    }
}
