use std::io::Write;
use std::path::{Path, PathBuf};

use crate::command::{
    CommandResult, input_error, language_for, lsp_error, output_error, parse_location,
    positive_usize,
};
use crate::language::Language;
use crate::lsp::{LspLocation, LspRange, LspService, file_path_from_uri, normalize_range};
use crate::lsp_options::LspOptions;
use crate::util::{
    absolute_lexical, display_path, escape_untrusted_text, quote_metadata, read_source,
};

const DEFAULT_DEFINITION_MAX_ITEMS: usize = 20;
const DEFAULT_REFERENCE_MAX_ITEMS: usize = 200;
const DEFAULT_HOVER_MAX_BYTES: usize = 16 * 1024;

struct SemanticTarget {
    path: PathBuf,
    language: Language,
    source: String,
    row: usize,
    byte_column: usize,
}

fn parse_semantic_target(
    value: &str,
    explicit: Option<Language>,
    cwd: &Path,
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
    let source = read_source(&path).map_err(input_error)?;
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
    language: Language,
    cwd: &Path,
    command: &str,
) -> Result<LspService, (i32, String)> {
    if !options.has_server(language) {
        return Err((
            2,
            format!(
                "{command} requires --lsp ABSOLUTE_SERVER_PATH (or --lsp {}=ABSOLUTE_SERVER_PATH)",
                language.name()
            ),
        ));
    }
    Ok(LspService::new(options.config(cwd)?))
}

#[derive(Clone, Copy)]
enum SemanticCommand {
    Definition,
    References,
    Hover,
}

impl SemanticCommand {
    const fn name(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::References => "references",
            Self::Hover => "hover",
        }
    }
}

struct SemanticOptions {
    target: String,
    max_items: usize,
    max_bytes: usize,
    include_declaration: bool,
}

fn parse_options(
    args: &[String],
    command: SemanticCommand,
) -> Result<SemanticOptions, (i32, String)> {
    let mut target = None;
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
                if target.replace(value.to_string()).is_some() {
                    return usage(format!(
                        "{} requires exactly one FILE:LINE:COLUMN target",
                        command.name()
                    ));
                }
                index += 1;
            }
        }
    }
    let target = target.ok_or_else(|| {
        (
            2,
            format!(
                "{} requires exactly one FILE:LINE:COLUMN target",
                command.name()
            ),
        )
    })?;
    Ok(SemanticOptions {
        target,
        max_items: max_items.unwrap_or(match command {
            SemanticCommand::Definition => DEFAULT_DEFINITION_MAX_ITEMS,
            SemanticCommand::References => DEFAULT_REFERENCE_MAX_ITEMS,
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
    let options = parse_options(args, SemanticCommand::Definition)?;
    let target = parse_semantic_target(&options.target, explicit, cwd)?;
    let mut service = semantic_service(lsp, target.language, cwd, "definition")?;
    let locations = service
        .definition(
            &target.path,
            target.language,
            &target.source,
            target.row,
            target.byte_column,
        )
        .map_err(lsp_error)?;
    render_locations(
        "definition",
        &options.target,
        &target,
        locations,
        options.max_items,
        cwd,
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
    let options = parse_options(args, SemanticCommand::References)?;
    let target = parse_semantic_target(&options.target, explicit, cwd)?;
    let mut service = semantic_service(lsp, target.language, cwd, "references")?;
    let locations = service
        .references(
            &target.path,
            target.language,
            &target.source,
            target.row,
            target.byte_column,
            options.include_declaration,
        )
        .map_err(lsp_error)?;
    render_locations(
        "references",
        &options.target,
        &target,
        locations,
        options.max_items,
        cwd,
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
    let options = parse_options(args, SemanticCommand::Hover)?;
    let target = parse_semantic_target(&options.target, explicit, cwd)?;
    let mut service = semantic_service(lsp, target.language, cwd, "hover")?;
    let hover = service
        .hover(
            &target.path,
            target.language,
            &target.source,
            target.row,
            target.byte_column,
        )
        .map_err(lsp_error)?;
    let Some(hover) = hover else {
        writeln!(
            output,
            "# pira_codenav hover target={} language={} backend=lsp available=0",
            quote_metadata(&options.target),
            target.language.name()
        )
        .map_err(output_error)?;
        return Ok(());
    };
    let (safe, escaped_controls) = escape_untrusted_text(&hover.contents);
    let (shown, truncated) = truncate_utf8(&safe, options.max_bytes);
    write!(
        output,
        "# pira_codenav hover target={} language={} backend=lsp available=1 format={} bytes={} total_bytes={} truncated={}",
        quote_metadata(&options.target),
        target.language.name(),
        hover.format.as_str(),
        shown.len(),
        safe.len(),
        usize::from(truncated)
    )
    .map_err(output_error)?;
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
    writeln!(
        output,
        "# pira_codenav {command} target={} language={} backend=lsp count={} shown={} omitted={}",
        quote_metadata(target_value),
        target.language.name(),
        count,
        shown,
        count.saturating_sub(shown)
    )
    .map_err(output_error)?;
    let mut last_source = None::<(PathBuf, Option<String>)>;
    for location in locations.into_iter().take(shown) {
        if let Some(path) = file_path_from_uri(&location.uri).map_err(lsp_error)? {
            let normalized = if path == target.path {
                Some(
                    normalize_range(&target.source, location.range, location.encoding)
                        .map_err(lsp_error)?,
                )
            } else {
                if last_source
                    .as_ref()
                    .is_none_or(|(cached, _)| cached != &path)
                {
                    last_source = Some((path.clone(), read_source(&path).ok()));
                }
                match last_source
                    .as_ref()
                    .and_then(|(_, source)| source.as_deref())
                {
                    Some(source) => Some(
                        normalize_range(source, location.range, location.encoding)
                            .map_err(lsp_error)?,
                    ),
                    None => None,
                }
            };
            if let Some(range) = normalized {
                writeln!(
                    output,
                    "location file={} range={}",
                    display_path(&path, cwd),
                    format_lsp_range(range)
                )
                .map_err(output_error)?;
            } else {
                writeln!(
                    output,
                    "location file={} lsp_range={} encoding={}",
                    display_path(&path, cwd),
                    format_lsp_range(location.range),
                    location.encoding.as_str()
                )
                .map_err(output_error)?;
            }
        } else {
            writeln!(
                output,
                "location uri={} lsp_range={} encoding={}",
                quote_metadata(&location.uri),
                format_lsp_range(location.range),
                location.encoding.as_str()
            )
            .map_err(output_error)?;
        }
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
