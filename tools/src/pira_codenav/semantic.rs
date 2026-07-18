use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::command::{
    CommandResult, input_error, language_for, lsp_error, output_error, parse_location,
    positive_usize,
};
use crate::language::Language;
use crate::lsp::{
    LspCall, LspLocation, LspRange, LspService, PositionEncoding, file_path_from_uri,
    normalize_range,
};
use crate::lsp_options::LspOptions;
use crate::util::{
    absolute_lexical, display_path, escape_untrusted_text, quote_metadata, read_source,
    sanitize_metadata,
};

const DEFAULT_DEFINITION_MAX_ITEMS: usize = 20;
const DEFAULT_REFERENCE_MAX_ITEMS: usize = 200;
const DEFAULT_CALL_MAX_ITEMS: usize = 100;
const DEFAULT_CALL_SITE_MAX_ITEMS: usize = 8;
const DEFAULT_HOVER_MAX_BYTES: usize = 16 * 1024;
const MAX_SEMANTIC_TARGETS: usize = 32;
const MAX_BATCH_ERRORS: usize = 16;

struct SemanticTarget {
    path: PathBuf,
    language: Language,
    source: Arc<str>,
    row: usize,
    byte_column: usize,
}

fn parse_semantic_target(
    value: &str,
    explicit: Option<Language>,
    cwd: &Path,
    sources: &mut BTreeMap<PathBuf, Arc<str>>,
) -> Result<SemanticTarget, (i32, String)> {
    let (path, line, column) = parse_location(value).ok_or_else(|| {
        (
            2,
            "semantic targets must use FILE:LINE:COLUMN with one-based UTF-8 byte coordinates"
                .into(),
        )
    })?;
    let column = column.ok_or_else(|| {
        (
            2,
            "semantic targets require FILE:LINE:COLUMN; a line alone is ambiguous".into(),
        )
    })?;
    if line == 0 || column == 0 {
        return Err((2, "semantic target line and column must be positive".into()));
    }
    let path = absolute_lexical(Path::new(path), cwd);
    let language = language_for(&path, explicit)?;
    let source = match sources.get(&path) {
        Some(source) => Arc::clone(source),
        None => {
            let source = Arc::<str>::from(read_source(&path).map_err(input_error)?);
            sources.insert(path.clone(), Arc::clone(&source));
            source
        }
    };
    Ok(SemanticTarget {
        path,
        language,
        source,
        row: line - 1,
        byte_column: column - 1,
    })
}

fn semantic_service(
    options: &LspOptions,
    targets: &[SemanticTarget],
    cwd: &Path,
    command: &str,
) -> Result<LspService, (i32, String)> {
    for target in targets {
        if !options.has_server(target.language) {
            return Err((
                2,
                format!(
                    "{command} requires an LSP for {}; install a conventional server on PATH or pass --lsp {}=ABSOLUTE_SERVER_PATH",
                    target.language.name(),
                    target.language.name()
                ),
            ));
        }
    }
    Ok(LspService::new_semantic(options.config(cwd)?))
}

#[derive(Clone, Copy)]
enum SemanticCommand {
    Definition,
    Implementation,
    TypeDefinition,
    References,
    Hover,
    Callers,
    Callees,
}

impl SemanticCommand {
    const fn name(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::Implementation => "implementation",
            Self::TypeDefinition => "type-definition",
            Self::References => "references",
            Self::Hover => "hover",
            Self::Callers => "callers",
            Self::Callees => "callees",
        }
    }
}

struct SemanticOptions {
    targets: Vec<String>,
    max_items: usize,
    max_bytes: usize,
    include_declaration: bool,
}

fn parse_options(
    args: &[String],
    command: SemanticCommand,
) -> Result<SemanticOptions, (i32, String)> {
    let mut targets = Vec::new();
    let mut max_items = None;
    let mut max_bytes = None;
    let mut include_declaration = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        match option {
            "--max-items" if !matches!(command, SemanticCommand::Hover) => {
                if max_items.is_some() {
                    return usage("--max-items may be specified only once");
                }
                max_items = Some(positive_usize(
                    args.get(index + 1)
                        .ok_or_else(|| (2, "--max-items requires a value".into()))?,
                    "--max-items",
                )?);
                index += 2;
            }
            "--max-bytes" if matches!(command, SemanticCommand::Hover) => {
                if max_bytes.is_some() {
                    return usage("--max-bytes may be specified only once");
                }
                max_bytes = Some(positive_usize(
                    args.get(index + 1)
                        .ok_or_else(|| (2, "--max-bytes requires a value".into()))?,
                    "--max-bytes",
                )?);
                index += 2;
            }
            "--include-declaration" if matches!(command, SemanticCommand::References) => {
                if include_declaration {
                    return usage("--include-declaration may be specified only once");
                }
                include_declaration = true;
                index += 1;
            }
            value if value.starts_with('-') => {
                return usage(format!("unknown {} option `{value}`", command.name()));
            }
            value => {
                if targets.len() >= MAX_SEMANTIC_TARGETS {
                    return usage(format!(
                        "{} accepts at most {MAX_SEMANTIC_TARGETS} FILE:LINE:COLUMN targets",
                        command.name()
                    ));
                }
                targets.push(value.to_string());
                index += 1;
            }
        }
    }
    if targets.is_empty() {
        return Err((
            2,
            format!(
                "{} requires at least one FILE:LINE:COLUMN target",
                command.name()
            ),
        ));
    }
    Ok(SemanticOptions {
        targets,
        max_items: max_items.unwrap_or(match command {
            SemanticCommand::Definition
            | SemanticCommand::Implementation
            | SemanticCommand::TypeDefinition => DEFAULT_DEFINITION_MAX_ITEMS,
            SemanticCommand::References => DEFAULT_REFERENCE_MAX_ITEMS,
            SemanticCommand::Callers | SemanticCommand::Callees => DEFAULT_CALL_MAX_ITEMS,
            SemanticCommand::Hover => 0,
        }),
        max_bytes: max_bytes.unwrap_or(DEFAULT_HOVER_MAX_BYTES),
        include_declaration,
    })
}

pub fn definition(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    run_command(
        args,
        SemanticCommand::Definition,
        explicit,
        cwd,
        lsp,
        output,
    )
}

pub fn implementation(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    run_command(
        args,
        SemanticCommand::Implementation,
        explicit,
        cwd,
        lsp,
        output,
    )
}

pub fn type_definition(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    run_command(
        args,
        SemanticCommand::TypeDefinition,
        explicit,
        cwd,
        lsp,
        output,
    )
}

pub fn references(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    run_command(
        args,
        SemanticCommand::References,
        explicit,
        cwd,
        lsp,
        output,
    )
}

pub fn hover(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    run_command(args, SemanticCommand::Hover, explicit, cwd, lsp, output)
}

pub fn callers(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    run_command(args, SemanticCommand::Callers, explicit, cwd, lsp, output)
}

pub fn callees(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    run_command(args, SemanticCommand::Callees, explicit, cwd, lsp, output)
}

fn run_command(
    args: &[String],
    command: SemanticCommand,
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    let options = parse_options(args, command)?;
    let mut sources = BTreeMap::new();
    let targets = options
        .targets
        .iter()
        .map(|value| parse_semantic_target(value, explicit, cwd, &mut sources))
        .collect::<Result<Vec<_>, _>>()?;
    let mut service = semantic_service(lsp, &targets, cwd, command.name())?;
    let mut succeeded = 0usize;
    let mut failures = Vec::new();
    let mut omitted_errors = 0usize;
    let mut first_failure = None;
    for (value, target) in options.targets.iter().zip(&targets) {
        match execute_one(command, &options, value, target, &mut service, cwd, output) {
            Ok(()) => succeeded += 1,
            Err((code, message)) if code <= 1 => return Err((code, message)),
            Err((code, message)) if targets.len() == 1 => return Err((code, message)),
            Err((code, message)) => {
                first_failure.get_or_insert((code, message.clone()));
                if failures.len() < MAX_BATCH_ERRORS {
                    failures.push((value, code, message));
                } else {
                    omitted_errors += 1;
                }
            }
        }
    }
    for (target, code, message) in &failures {
        writeln!(
            output,
            "# pira_codenav {} error target={} code={} message={}",
            command.name(),
            quote_metadata(target),
            code,
            quote_metadata(message)
        )
        .map_err(output_error)?;
    }
    if targets.len() > 1 {
        write!(
            output,
            "# pira_codenav {} batch targets={} succeeded={}",
            command.name(),
            targets.len(),
            succeeded
        )
        .map_err(output_error)?;
        let failed = targets.len().saturating_sub(succeeded);
        if failed > 0 {
            write!(output, " failed={failed} complete=0").map_err(output_error)?;
        }
        if omitted_errors > 0 {
            write!(output, " errors_omitted={omitted_errors}").map_err(output_error)?;
        }
        writeln!(output).map_err(output_error)?;
    }
    if succeeded == 0 {
        return Err(
            first_failure.unwrap_or_else(|| (3, format!("all {} targets failed", command.name())))
        );
    }
    Ok(())
}

fn execute_one(
    command: SemanticCommand,
    options: &SemanticOptions,
    value: &str,
    target: &SemanticTarget,
    service: &mut LspService,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    match command {
        SemanticCommand::Definition
        | SemanticCommand::Implementation
        | SemanticCommand::TypeDefinition
        | SemanticCommand::References => {
            let locations = match command {
                SemanticCommand::Definition => service.definition(
                    &target.path,
                    target.language,
                    &target.source,
                    target.row,
                    target.byte_column,
                ),
                SemanticCommand::Implementation => service.implementation(
                    &target.path,
                    target.language,
                    &target.source,
                    target.row,
                    target.byte_column,
                ),
                SemanticCommand::TypeDefinition => service.type_definition(
                    &target.path,
                    target.language,
                    &target.source,
                    target.row,
                    target.byte_column,
                ),
                SemanticCommand::References => service.references(
                    &target.path,
                    target.language,
                    &target.source,
                    target.row,
                    target.byte_column,
                    options.include_declaration,
                ),
                _ => unreachable!(),
            }
            .map_err(lsp_error)?;
            render_locations(
                command.name(),
                value,
                target,
                locations,
                options.max_items,
                cwd,
                output,
            )
        }
        SemanticCommand::Hover => {
            let hover = service
                .hover(
                    &target.path,
                    target.language,
                    &target.source,
                    target.row,
                    target.byte_column,
                )
                .map_err(lsp_error)?;
            render_hover(value, target, hover, options.max_bytes, output)
        }
        SemanticCommand::Callers | SemanticCommand::Callees => {
            let calls = service
                .calls(
                    &target.path,
                    target.language,
                    &target.source,
                    target.row,
                    target.byte_column,
                    matches!(command, SemanticCommand::Callers),
                )
                .map_err(lsp_error)?;
            render_calls(
                command.name(),
                value,
                target,
                calls,
                options.max_items,
                cwd,
                output,
            )
        }
    }
}

fn render_hover(
    target_value: &str,
    target: &SemanticTarget,
    hover: Option<crate::lsp::LspHover>,
    max_bytes: usize,
    output: &mut dyn Write,
) -> CommandResult {
    let Some(hover) = hover else {
        writeln!(
            output,
            "# pira_codenav hover target={} available=0",
            quote_metadata(target_value)
        )
        .map_err(output_error)?;
        return Ok(());
    };
    let (safe, escaped_controls) = escape_untrusted_text(&hover.contents);
    let (shown, truncated) = truncate_utf8(&safe, max_bytes);
    write!(
        output,
        "# pira_codenav hover target={} format={}",
        quote_metadata(target_value),
        hover.format.as_str()
    )
    .map_err(output_error)?;
    if truncated {
        write!(
            output,
            " shown_bytes={} total_bytes={} truncated=1",
            shown.len(),
            safe.len()
        )
        .map_err(output_error)?;
    }
    if let Some(range) = hover.range {
        let range = normalize_range(&target.source, range, hover.encoding).map_err(lsp_error)?;
        write!(output, " range={}", format_lsp_range(range)).map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    if escaped_controls == 0 {
        writeln!(output, "--- begin untrusted LSP hover ---").map_err(output_error)?;
    } else {
        writeln!(
            output,
            "--- begin untrusted LSP hover controls_escaped={escaped_controls} ---"
        )
        .map_err(output_error)?;
    }
    output.write_all(shown.as_bytes()).map_err(output_error)?;
    if !shown.ends_with('\n') {
        writeln!(output).map_err(output_error)?;
    }
    writeln!(output, "--- end LSP hover ---").map_err(output_error)
}

fn render_calls(
    command: &str,
    target_value: &str,
    target: &SemanticTarget,
    calls: Vec<LspCall>,
    max_items: usize,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    let count = calls.len();
    let shown = count.min(max_items);
    write!(
        output,
        "# pira_codenav {command} target={} count={}",
        quote_metadata(target_value),
        count
    )
    .map_err(output_error)?;
    if shown != count {
        write!(output, " shown={} omitted={}", shown, count - shown).map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    let mut last_source = None::<(PathBuf, Option<String>)>;
    for call in calls.into_iter().take(shown) {
        write!(
            output,
            "call name={} kind={}",
            quote_metadata(&call.name),
            call.kind
        )
        .map_err(output_error)?;
        render_location_fields(
            &call.uri,
            call.range,
            call.encoding,
            target,
            cwd,
            &mut last_source,
            output,
        )?;
        let site_count = call.call_ranges.len();
        let sites_shown = site_count.min(DEFAULT_CALL_SITE_MAX_ITEMS);
        if sites_shown > 0 {
            let mut rendered = Vec::with_capacity(sites_shown);
            for range in call.call_ranges.into_iter().take(sites_shown) {
                rendered.push(render_site_range(
                    &call.site_uri,
                    range,
                    call.encoding,
                    target,
                    cwd,
                    &mut last_source,
                )?);
            }
            write!(output, " callsites={}", quote_metadata(&rendered.join(",")))
                .map_err(output_error)?;
        }
        let sites_omitted = site_count.saturating_sub(sites_shown);
        if sites_omitted > 0 {
            write!(output, " callsites_omitted={sites_omitted}").map_err(output_error)?;
        }
        writeln!(output).map_err(output_error)?;
    }
    Ok(())
}

fn render_site_range(
    uri: &str,
    range: LspRange,
    encoding: PositionEncoding,
    target: &SemanticTarget,
    cwd: &Path,
    last_source: &mut Option<(PathBuf, Option<String>)>,
) -> Result<String, (i32, String)> {
    let Some(path) = file_path_from_uri(uri).map_err(lsp_error)? else {
        return Ok(format!(
            "{}:{}:{}",
            sanitize_metadata(uri),
            format_lsp_range(range),
            encoding.as_str()
        ));
    };
    let normalized = if path == target.path {
        Some(normalize_range(&target.source, range, encoding).map_err(lsp_error)?)
    } else {
        if last_source
            .as_ref()
            .is_none_or(|(cached, _)| cached != &path)
        {
            *last_source = Some((path.clone(), read_source(&path).ok()));
        }
        last_source
            .as_ref()
            .and_then(|(_, source)| source.as_deref())
            .map(|source| normalize_range(source, range, encoding))
            .transpose()
            .map_err(lsp_error)?
    };
    Ok(match normalized {
        Some(range) => format!("{}:{}", display_path(&path, cwd), format_lsp_range(range)),
        None => format!(
            "{}:{}:{}",
            display_path(&path, cwd),
            format_lsp_range(range),
            encoding.as_str()
        ),
    })
}

fn render_locations(
    command: &str,
    target_value: &str,
    target: &SemanticTarget,
    locations: Vec<LspLocation>,
    max_items: usize,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    let count = locations.len();
    let shown = count.min(max_items);
    write!(
        output,
        "# pira_codenav {command} target={} count={}",
        quote_metadata(target_value),
        count
    )
    .map_err(output_error)?;
    if shown != count {
        write!(output, " shown={} omitted={}", shown, count - shown).map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    let mut last_source = None::<(PathBuf, Option<String>)>;
    for location in locations.into_iter().take(shown) {
        write!(output, "location").map_err(output_error)?;
        render_location_fields(
            &location.uri,
            location.range,
            location.encoding,
            target,
            cwd,
            &mut last_source,
            output,
        )?;
        writeln!(output).map_err(output_error)?;
    }
    Ok(())
}

fn render_location_fields(
    uri: &str,
    range: LspRange,
    encoding: PositionEncoding,
    target: &SemanticTarget,
    cwd: &Path,
    last_source: &mut Option<(PathBuf, Option<String>)>,
    output: &mut dyn Write,
) -> CommandResult {
    if let Some(path) = file_path_from_uri(uri).map_err(lsp_error)? {
        let normalized = if path == target.path {
            Some(normalize_range(&target.source, range, encoding).map_err(lsp_error)?)
        } else {
            if last_source
                .as_ref()
                .is_none_or(|(cached, _)| cached != &path)
            {
                *last_source = Some((path.clone(), read_source(&path).ok()));
            }
            match last_source
                .as_ref()
                .and_then(|(_, source)| source.as_deref())
            {
                Some(source) => Some(normalize_range(source, range, encoding).map_err(lsp_error)?),
                None => None,
            }
        };
        if let Some(range) = normalized {
            write!(
                output,
                " file={} range={}",
                display_path(&path, cwd),
                format_lsp_range(range)
            )
            .map_err(output_error)?;
        } else {
            write!(
                output,
                " file={} lsp_range={} encoding={}",
                display_path(&path, cwd),
                format_lsp_range(range),
                encoding.as_str()
            )
            .map_err(output_error)?;
        }
    } else {
        write!(
            output,
            " uri={} lsp_range={} encoding={}",
            quote_metadata(uri),
            format_lsp_range(range),
            encoding.as_str()
        )
        .map_err(output_error)?;
    }
    Ok(())
}

fn format_lsp_range(range: LspRange) -> String {
    format!(
        "L{}:{}-{}:{}",
        range.start.line + 1,
        range.start.character + 1,
        range.end.line + 1,
        range.end.character + 1
    )
}

fn truncate_utf8(value: &str, max_bytes: usize) -> (&str, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], true)
}

fn usage<T, M: Into<String>>(message: M) -> Result<T, (i32, String)> {
    Err((2, message.into()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::parse_semantic_target;

    #[test]
    fn repeated_targets_share_one_source_allocation() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pira-codenav-semantic-{}-{unique}.py",
            std::process::id()
        ));
        fs::write(&path, "value = target()\n").expect("write temporary source");
        let value = format!("{}:1:9", path.display());
        let mut sources = BTreeMap::new();
        let first = parse_semantic_target(&value, None, &std::env::temp_dir(), &mut sources)
            .expect("first target");
        let second = parse_semantic_target(&value, None, &std::env::temp_dir(), &mut sources)
            .expect("second target");
        assert!(Arc::ptr_eq(&first.source, &second.source));
        assert_eq!(sources.len(), 1);
        fs::remove_file(path).expect("remove temporary source");
    }
}
