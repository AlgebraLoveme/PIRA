use std::collections::{BTreeMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rayon::prelude::*;

use crate::deps;
use crate::language::Language;
use crate::model::{ParseState, Symbol};
use crate::parse::{ParsedFile, parse_file};
use crate::util::{
    DEFAULT_MAX_ITEMS, absolute_lexical, display_path, hash16, percent_decode, quote_metadata,
    read_source,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_SHOW_MAX_ITEMS: usize = 20;
const DEFAULT_SHOW_MAX_BYTES: usize = 64 * 1024;

pub fn run<I>(arguments: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    let mut values = match arguments
        .into_iter()
        .map(|value| {
            value
                .into_string()
                .map_err(|_| "arguments must be valid UTF-8")
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(values) => values,
        Err(error) => return fail(2, error),
    };
    if !values.is_empty() {
        values.remove(0);
    }
    if values.first().is_some_and(|value| value == "help") && values.len() > 1 {
        return finish_output(print_command_help(&values[1], &mut output), &mut output);
    }
    if values.is_empty() || matches!(values[0].as_str(), "--help" | "-h" | "help") {
        return finish_output(print_global_help(&mut output), &mut output);
    }
    if matches!(values[0].as_str(), "--version" | "-V") {
        let result = writeln!(output, "pira_codenav {VERSION}").map_err(output_error);
        return finish_output(result, &mut output);
    }
    if values[0] == "languages" {
        if values.len() == 2 && matches!(values[1].as_str(), "--help" | "-h") {
            return finish_output(print_command_help("languages", &mut output), &mut output);
        }
        if values.len() != 1 {
            return fail(
                2,
                "languages accepts no arguments; run pira_codenav languages --help",
            );
        }
        return finish_output(print_languages(&mut output), &mut output);
    }

    let explicit_language = Language::parse_name(&values[0]);
    if let Some(language) = explicit_language {
        values.remove(0);
        if values.is_empty() || matches!(values[0].as_str(), "--help" | "-h") {
            return finish_output(print_language_help(language, &mut output), &mut output);
        }
    }
    if values.is_empty() {
        return fail(2, "missing subcommand");
    }
    let command = values.remove(0);
    if values.len() == 1 && matches!(values[0].as_str(), "--help" | "-h") {
        return finish_output(print_command_help(&command, &mut output), &mut output);
    }
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => return fail(2, format!("cannot determine current directory: {error}")),
    };
    let result = match command.as_str() {
        "outline" => command_outline(&values, explicit_language, &cwd, &mut output),
        "show" => command_show(&values, explicit_language, &cwd, &mut output),
        "map" => command_map(&values, explicit_language, &cwd, &mut output),
        "imports" => command_imports(&values, explicit_language, &cwd, &mut output),
        "dependents" => command_dependents(&values, explicit_language, &cwd, &mut output),
        "deps" => command_deps(&values, explicit_language, &cwd, &mut output),
        other => Err((
            2,
            format!("unknown subcommand `{other}`; run pira_codenav --help"),
        )),
    };
    match result {
        Ok(()) => finish_output(Ok(()), &mut output),
        Err((0, _)) => 0,
        Err((code, message)) => fail(code, message),
    }
}

type CommandResult = Result<(), (i32, String)>;

fn command_outline(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    let options = parse_outline_options(args)?;
    if options.paths.is_empty() {
        return usage("outline requires at least one file");
    }
    let total = options.paths.len();
    let mut failures = 0;
    for path in options.paths {
        let absolute = absolute_lexical(Path::new(&path), cwd);
        let result = (|| {
            let language = language_for(&absolute, explicit)?;
            let parsed = parse_file(&absolute, language).map_err(input_error)?;
            render_outline(
                &parsed,
                cwd,
                options.max_items,
                options.selectors,
                options.signatures,
                &options.matches,
                output,
            )?;
            warn_partial(&parsed);
            Ok(())
        })();
        if let Err((code, message)) = result {
            if total == 1 {
                return Err((code, message));
            }
            failures += 1;
            writeln!(
                output,
                "# pira_codenav outline error file={} code={} message={}",
                quote_metadata(&path),
                code,
                quote_metadata(&message)
            )
            .map_err(output_error)?;
        }
    }
    if failures == total {
        output.flush().map_err(output_error)?;
        return Err((
            3,
            "all outline files failed; inspect the reported file errors".into(),
        ));
    }
    Ok(())
}

struct OutlineOptions {
    paths: Vec<String>,
    max_items: usize,
    selectors: bool,
    signatures: bool,
    matches: Vec<String>,
}

fn parse_outline_options(args: &[String]) -> Result<OutlineOptions, (i32, String)> {
    let mut paths = Vec::new();
    let mut max_items = DEFAULT_MAX_ITEMS;
    let mut selectors = false;
    let mut signatures = false;
    let mut matches = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if matches!(option, "--max-items" | "--match") {
            let value = args
                .get(index + 1)
                .ok_or_else(|| (2, format!("{option} requires a value")))?;
            if option == "--max-items" {
                max_items = positive_usize(value, option)?;
            } else if value.is_empty() {
                return Err((2, "--match requires a non-empty value".into()));
            } else {
                matches.push(value.to_lowercase());
            }
            index += 2;
        } else if option == "--selectors" {
            selectors = true;
            index += 1;
        } else if option == "--signatures" {
            signatures = true;
            index += 1;
        } else if option.starts_with('-') {
            return Err((
                2,
                format!("unknown option `{option}`; run pira_codenav outline --help"),
            ));
        } else {
            paths.push(args[index].clone());
            index += 1;
        }
    }
    Ok(OutlineOptions {
        paths,
        max_items,
        selectors,
        signatures,
        matches,
    })
}

fn command_show(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    let options = parse_show_options(args)?;
    if options.targets.len() == 1
        && let Some((path_text, start, end)) = parse_line_range(&options.targets[0])
    {
        let path = absolute_lexical(Path::new(path_text), cwd);
        let language = language_for(&path, explicit)?;
        let mut item = Vec::new();
        render_line_range(&path, language, start, end, cwd, &mut item)?;
        if let Some(max_bytes) = options.max_bytes
            && item.len() > max_bytes
        {
            writeln!(
                output,
                "# pira_codenav show targets=1 shown=0 failed=0 duplicates=0 omitted=1 byte_limited=1 payload_bytes=0 max_items={} max_bytes={}",
                options.max_items.unwrap_or(DEFAULT_SHOW_MAX_ITEMS),
                max_bytes
            )
            .map_err(output_error)?;
            return Ok(());
        }
        output.write_all(&item).map_err(output_error)?;
        return Ok(());
    }
    if options.targets.len() == 1 && options.max_bytes.is_none() {
        let (parsed, symbol_index) = resolve_show_target(&options.targets[0], explicit, cwd)?;
        render_source(&parsed, &parsed.symbols[symbol_index], cwd, output)?;
        warn_partial(&parsed);
        return Ok(());
    }

    let max_items = options.max_items.unwrap_or(DEFAULT_SHOW_MAX_ITEMS);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_SHOW_MAX_BYTES);
    let mut rendered = Vec::new();
    let mut identities = HashSet::new();
    let mut duplicates = 0;
    let mut byte_limited = 0;
    let mut failures = Vec::new();
    let mut considered = 0;
    let mut payload_bytes = 0;
    for target in &options.targets {
        if considered >= max_items {
            break;
        }
        let (parsed, symbol_index) = match resolve_show_target(target, explicit, cwd) {
            Ok(resolved) => resolved,
            Err((code, message)) => {
                failures.push(ShowFailure {
                    target: target.clone(),
                    code,
                    message,
                });
                continue;
            }
        };
        let symbol = &parsed.symbols[symbol_index];
        let identity = (parsed.path.clone(), symbol.start_byte, symbol.end_byte);
        if !identities.insert(identity) {
            duplicates += 1;
            continue;
        }
        considered += 1;
        let mut item = Vec::new();
        render_source(&parsed, symbol, cwd, &mut item)?;
        warn_partial(&parsed);
        if item.len() > max_bytes.saturating_sub(payload_bytes) {
            byte_limited += 1;
            continue;
        }
        payload_bytes += item.len();
        rendered.push(item);
    }
    let omitted = options
        .targets
        .len()
        .saturating_sub(rendered.len() + duplicates + failures.len());
    writeln!(
        output,
        "# pira_codenav show targets={} shown={} failed={} duplicates={} omitted={} byte_limited={} payload_bytes={} max_items={} max_bytes={}",
        options.targets.len(),
        rendered.len(),
        failures.len(),
        duplicates,
        omitted,
        byte_limited,
        payload_bytes,
        max_items,
        max_bytes
    )
    .map_err(output_error)?;
    for failure in &failures {
        writeln!(
            output,
            "error target={} code={} message={}",
            quote_metadata(&failure.target),
            failure.code,
            quote_metadata(&failure.message)
        )
        .map_err(output_error)?;
    }
    for item in rendered {
        output.write_all(&item).map_err(output_error)?;
    }
    if failures.len() == options.targets.len() {
        output.flush().map_err(output_error)?;
        return Err((
            3,
            "all show targets failed; inspect the reported target errors".into(),
        ));
    }
    Ok(())
}

struct ShowFailure {
    target: String,
    code: i32,
    message: String,
}

struct ShowOptions {
    targets: Vec<String>,
    max_items: Option<usize>,
    max_bytes: Option<usize>,
}

fn parse_show_options(args: &[String]) -> Result<ShowOptions, (i32, String)> {
    let mut targets = Vec::new();
    let mut max_items = None;
    let mut max_bytes = None;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if matches!(option, "--max-items" | "--max-bytes") {
            let value = args
                .get(index + 1)
                .ok_or_else(|| (2, format!("{option} requires a positive integer")))?;
            let parsed = positive_usize(value, option)?;
            if option == "--max-items" {
                max_items = Some(parsed);
            } else {
                max_bytes = Some(parsed);
            }
            index += 2;
        } else if option.starts_with('-') {
            return Err((
                2,
                format!(
                    "unknown option `{option}`; pass each symbol as a direct target such as file::qualified-name; run pira_codenav show --help"
                ),
            ));
        } else {
            targets.push(args[index].clone());
            index += 1;
        }
    }
    if targets.is_empty() {
        return Err((
            2,
            "show requires at least one selector, file:line[:column], or file::symbol".into(),
        ));
    }
    Ok(ShowOptions {
        targets,
        max_items,
        max_bytes,
    })
}

fn resolve_show_target(
    target: &str,
    explicit: Option<Language>,
    cwd: &Path,
) -> Result<(ParsedFile, usize), (i32, String)> {
    let (path, language, selector) = if target.starts_with("pira://") {
        let decoded = parse_selector(target).map_err(input_error)?;
        if explicit.is_some_and(|value| value != decoded.language) {
            return Err((
                2,
                "language mismatch between explicit language and selector".into(),
            ));
        }
        (
            absolute_lexical(Path::new(&decoded.path), cwd),
            decoded.language,
            Some(decoded),
        )
    } else {
        let path = target_path(target, cwd).ok_or_else(|| {
            (
                2,
                format!(
                    "invalid show target `{target}`; use file:line[:column], file::qualified-name, or an outline selector"
                ),
            )
        })?;
        let language = language_for(&path, explicit)?;
        (path, language, None)
    };
    let parsed = parse_file(&path, language).map_err(input_error)?;
    let symbol_index = if let Some(selector) = &selector {
        parsed
            .symbols
            .iter()
            .position(|symbol| {
                symbol.kind == selector.kind
                    && symbol.qualified_name == selector.qualified
                    && hash16(
                        parsed
                            .source
                            .get(symbol.start_byte..symbol.end_byte)
                            .unwrap_or_default()
                            .as_bytes(),
                    ) == selector.hash
            })
            .ok_or_else(|| {
                (
                    4,
                    format!(
                        "stale selector: symbol or source version no longer exists: {}",
                        selector.qualified
                    ),
                )
            })?
    } else if let Some((_, qualified)) = split_existing_symbol_target(target, cwd) {
        let exact = parsed
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, symbol)| symbol.qualified_name == qualified)
            .collect::<Vec<_>>();
        let matches = if exact.is_empty() {
            parsed
                .symbols
                .iter()
                .enumerate()
                .filter(|(_, symbol)| qualified_suffix_matches(&symbol.qualified_name, &qualified))
                .collect::<Vec<_>>()
        } else {
            exact
        };
        match matches.as_slice() {
            [] => return Err((3, format!("symbol not found: {qualified}"))),
            [(index, _)] => *index,
            _ => {
                let locations = matches
                    .iter()
                    .take(8)
                    .map(|(_, symbol)| {
                        format!(
                            "{}:{}",
                            display_path(&parsed.path, cwd),
                            symbol.start_row + 1
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err((
                    3,
                    format!(
                        "ambiguous symbol `{qualified}`: {} matches at {locations}; use a location or selector",
                        matches.len()
                    ),
                ));
            }
        }
    } else {
        let (_, line, column) = parse_location(target)
            .ok_or_else(|| (2, format!("invalid source location: {target}")))?;
        parsed
            .symbols
            .iter()
            .enumerate()
            .filter(|symbol| {
                column.map_or_else(
                    || symbol.1.contains_line(line),
                    |column| symbol.1.contains_position(line, column),
                )
            })
            .min_by_key(|(_, symbol)| symbol.byte_len())
            .map(|(index, _)| index)
            .ok_or_else(|| (3, format!("no named source item contains line {line}")))?
    };
    Ok((parsed, symbol_index))
}

fn qualified_suffix_matches(candidate: &str, query: &str) -> bool {
    candidate == query
        || candidate
            .strip_suffix(query)
            .is_some_and(|prefix| prefix.ends_with('.') || prefix.ends_with("::"))
}

fn command_map(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    let (paths, max_items, _) = parse_paths_and_limit(args, false)?;
    if paths.len() != 1 {
        return usage("map requires exactly one directory");
    }
    let root = absolute_lexical(Path::new(&paths[0]), cwd);
    if !root.is_dir() {
        return Err(input_error(format!(
            "map target is not a directory: {}",
            root.display()
        )));
    }
    let discovery = discover_files(&root, explicit);
    let parsed = discovery
        .files
        .par_iter()
        .map(|(path, language)| {
            parse_file(path, *language).map(|parsed| FileSummary {
                path: parsed.path,
                language: parsed.language,
                state: parsed.state,
                names: top_level_map_names(&parsed.symbols),
            })
        })
        .collect::<Vec<_>>();
    let failed = parsed.iter().filter(|result| result.is_err()).count();
    let summaries = parsed
        .into_iter()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    let parsed_count = summaries.len();
    let ok = summaries
        .iter()
        .filter(|summary| summary.state == ParseState::Ok)
        .count();
    let recovered = summaries
        .iter()
        .filter(|summary| summary.state == ParseState::Recovered)
        .count();
    let partial = summaries
        .iter()
        .filter(|summary| summary.state == ParseState::Partial)
        .count();
    let shown_summaries = balanced_summaries(summaries, &root, max_items);
    let shown = shown_summaries.len();
    writeln!(
        output,
        "# pira_codenav map root={} discovered={} eligible={} parsed={} ok={} recovered={} partial={} failed={} unsupported={} ambiguous={} shown={} omitted={}",
        display_path(&root, cwd),
        discovery.discovered,
        discovery.files.len(),
        parsed_count,
        ok,
        recovered,
        partial,
        failed,
        discovery.unsupported,
        discovery.ambiguous,
        shown,
        parsed_count.saturating_sub(shown)
    )
    .map_err(output_error)?;
    for file in shown_summaries {
        let names = file.names.join(",");
        writeln!(
            output,
            "file={} language={} parse={} symbols={}",
            display_path(&file.path, cwd),
            file.language.name(),
            file.state.as_str(),
            names
        )
        .map_err(output_error)?;
    }
    Ok(())
}

struct FileSummary {
    path: PathBuf,
    language: Language,
    state: ParseState,
    names: Vec<String>,
}

fn top_level_map_names(symbols: &[Symbol]) -> Vec<String> {
    let mut names = Vec::with_capacity(12);
    for symbol in symbols
        .iter()
        .filter(|symbol| symbol.depth == 0 && symbol.kind != "binding")
        .chain(
            symbols
                .iter()
                .filter(|symbol| symbol.depth == 0 && symbol.kind == "binding"),
        )
        .take(12)
    {
        names.push(compact_map_name(symbol.qualified_name.clone()));
    }
    names
}

fn compact_map_name(mut name: String) -> String {
    const MAX_BYTES: usize = 96;
    if name.len() <= MAX_BYTES {
        return name;
    }
    let mut end = MAX_BYTES;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name.truncate(end);
    name.push('…');
    name
}

fn balanced_summaries(
    summaries: Vec<FileSummary>,
    root: &Path,
    max_items: usize,
) -> Vec<FileSummary> {
    let mut groups = BTreeMap::<PathBuf, VecDeque<FileSummary>>::new();
    for summary in summaries {
        let directory = summary
            .path
            .strip_prefix(root)
            .ok()
            .and_then(Path::parent)
            .unwrap_or(Path::new(""))
            .to_path_buf();
        groups.entry(directory).or_default().push_back(summary);
    }
    for group in groups.values_mut() {
        group
            .make_contiguous()
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
    let mut selected = Vec::with_capacity(max_items.min(groups.len()));
    while selected.len() < max_items {
        let mut advanced = false;
        for group in groups.values_mut() {
            if let Some(summary) = group.pop_front() {
                selected.push(summary);
                advanced = true;
                if selected.len() == max_items {
                    break;
                }
            }
        }
        if !advanced {
            break;
        }
    }
    selected
}

fn command_imports(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    if args.is_empty() {
        return usage("imports requires at least one file");
    }
    let mut failures = 0;
    for value in args {
        let path = absolute_lexical(Path::new(value), cwd);
        let result = (|| {
            let language = language_for(&path, explicit)?;
            let parsed = parse_file(&path, language).map_err(input_error)?;
            let edges = deps::imports(&parsed, cwd);
            writeln!(
                output,
                "# pira_codenav imports file={} language={} count={}",
                display_path(&path, cwd),
                language.name(),
                edges.len()
            )
            .map_err(output_error)?;
            for edge in edges {
                writeln!(
                    output,
                    "import line={} target={} resolution={} text={}",
                    edge.line,
                    edge.target_label,
                    edge.resolution,
                    quote_metadata(&edge.text)
                )
                .map_err(output_error)?;
            }
            warn_partial(&parsed);
            Ok(())
        })();
        if let Err((code, message)) = result {
            if args.len() == 1 {
                return Err((code, message));
            }
            failures += 1;
            writeln!(
                output,
                "# pira_codenav imports error file={} code={} message={}",
                quote_metadata(value),
                code,
                quote_metadata(&message)
            )
            .map_err(output_error)?;
        }
    }
    if failures == args.len() {
        output.flush().map_err(output_error)?;
        return Err((
            3,
            "all imports files failed; inspect the reported file errors".into(),
        ));
    }
    Ok(())
}

fn command_dependents(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    let (target_value, root) = parse_rooted_target(args, cwd, "dependents")?;
    if !root.is_dir() {
        return Err(input_error(format!(
            "dependency root is not a directory: {}",
            root.display()
        )));
    }
    let target = absolute_lexical(Path::new(&target_value), &root);
    if !target.starts_with(&root) {
        return Err(input_error("dependency target must be inside --root"));
    }
    let target_language = language_for(&target, explicit)?;
    let discovery = discover_files(&root, explicit.or(Some(target_language)));
    let mut edges = discovery
        .files
        .par_iter()
        .filter(|(path, _)| *path != target)
        .filter_map(|(path, language)| {
            let parsed = parse_file(path, *language).ok()?;
            Some(deps::imports(&parsed, &root))
        })
        .flatten()
        .filter(|edge| edge.target.as_deref() == Some(target.as_path()))
        .collect::<Vec<_>>();
    edges.sort_by_key(|edge| (display_path(&edge.source, &root), edge.line));
    writeln!(
        output,
        "# pira_codenav dependents target={} root={} language={} count={}",
        display_path(&target, &root),
        display_path(&root, cwd),
        target_language.name(),
        edges.len()
    )
    .map_err(output_error)?;
    for edge in edges {
        writeln!(
            output,
            "dependent={} line={} target={} resolution={} import={}",
            display_path(&edge.source, &root),
            edge.line,
            edge.target_label,
            edge.resolution,
            quote_metadata(&edge.text)
        )
        .map_err(output_error)?;
    }
    Ok(())
}

fn parse_rooted_target(
    args: &[String],
    cwd: &Path,
    command: &str,
) -> Result<(String, PathBuf), (i32, String)> {
    let mut target = None;
    let mut root = cwd.to_path_buf();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--root" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| (2, "--root requires a directory".into()))?;
            root = absolute_lexical(Path::new(value), cwd);
            index += 2;
        } else if args[index].starts_with('-') {
            return Err((2, format!("unknown option `{}`", args[index])));
        } else if target.replace(args[index].clone()).is_some() {
            return Err((2, format!("{command} requires exactly one file target")));
        } else {
            index += 1;
        }
    }
    let target =
        target.ok_or_else(|| (2, format!("{command} requires exactly one file target")))?;
    Ok((target, root))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DependencyDirection {
    Imports,
    Dependents,
    Both,
}

impl DependencyDirection {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "imports" => Some(Self::Imports),
            "dependents" => Some(Self::Dependents),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Imports => "imports",
            Self::Dependents => "dependents",
            Self::Both => "both",
        }
    }
}

struct DependencyTraversal {
    depth: usize,
    direction: &'static str,
    source: PathBuf,
    target: PathBuf,
    line: usize,
}

struct LocalDependencyEdge {
    source: PathBuf,
    target: PathBuf,
    line: usize,
}

struct DependencyOptions {
    target: String,
    root: PathBuf,
    direction: DependencyDirection,
    depth: usize,
    max_items: usize,
}

fn command_deps(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    let options = parse_dependency_options(args, cwd)?;
    if !options.root.is_dir() {
        return Err(input_error(format!(
            "dependency root is not a directory: {}",
            options.root.display()
        )));
    }
    let target = absolute_lexical(Path::new(&options.target), &options.root);
    if !target.is_file() {
        if let Some((path, _, _)) = parse_location(&options.target)
            && absolute_lexical(Path::new(path), &options.root).is_file()
        {
            return Err(input_error(
                "deps requires a file path, not a source location; rerun with the line suffix removed or run pira_codenav deps --help",
            ));
        }
        return Err(input_error(format!(
            "dependency target is not a file: {}; run pira_codenav deps --help",
            target.display()
        )));
    }
    if !target.starts_with(&options.root) {
        return Err(input_error("dependency target must be inside --root"));
    }
    let target_language = language_for(&target, explicit)?;
    let discovery = discover_files(&options.root, explicit.or(Some(target_language)));
    let extracted = discovery
        .files
        .par_iter()
        .map(|(path, language)| {
            parse_file(path, *language).map(|parsed| {
                deps::imports(&parsed, &options.root)
                    .into_iter()
                    .filter_map(|edge| {
                        edge.target.map(|target| LocalDependencyEdge {
                            source: edge.source,
                            target,
                            line: edge.line,
                        })
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    let failed = extracted.iter().filter(|item| item.is_err()).count();
    let edges = extracted
        .into_iter()
        .filter_map(Result::ok)
        .flatten()
        .collect::<Vec<_>>();
    let mut traversed = Vec::new();
    if matches!(
        options.direction,
        DependencyDirection::Imports | DependencyDirection::Both
    ) {
        traverse_dependencies(&edges, &target, true, options.depth, &mut traversed);
    }
    if matches!(
        options.direction,
        DependencyDirection::Dependents | DependencyDirection::Both
    ) {
        traverse_dependencies(&edges, &target, false, options.depth, &mut traversed);
    }
    let shown = traversed.len().min(options.max_items);
    writeln!(
        output,
        "# pira_codenav deps target={} root={} language={} direction={} depth={} files={} failed={} edges={} shown={} omitted={}",
        display_path(&target, &options.root),
        display_path(&options.root, cwd),
        target_language.name(),
        options.direction.as_str(),
        options.depth,
        discovery.files.len(),
        failed,
        traversed.len(),
        shown,
        traversed.len().saturating_sub(shown)
    )
    .map_err(output_error)?;
    for edge in traversed.iter().take(shown) {
        writeln!(
            output,
            "edge depth={} direction={} from={} to={} line={}",
            edge.depth,
            edge.direction,
            display_path(&edge.source, &options.root),
            display_path(&edge.target, &options.root),
            edge.line
        )
        .map_err(output_error)?;
    }
    Ok(())
}

fn parse_dependency_options(
    args: &[String],
    cwd: &Path,
) -> Result<DependencyOptions, (i32, String)> {
    let mut target = None;
    let mut root = cwd.to_path_buf();
    let mut direction = DependencyDirection::Both;
    let mut depth = 2;
    let mut max_items = DEFAULT_MAX_ITEMS;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if matches!(option, "--root" | "--direction" | "--depth" | "--max-items") {
            let value = args
                .get(index + 1)
                .ok_or_else(|| (2, format!("{option} requires a value")))?;
            match option {
                "--root" => root = absolute_lexical(Path::new(value), cwd),
                "--direction" => {
                    direction = DependencyDirection::parse(value).ok_or_else(|| {
                        (2, "--direction must be imports, dependents, or both".into())
                    })?;
                }
                "--depth" => {
                    depth = positive_usize(value, "--depth")?;
                }
                "--max-items" => {
                    max_items = positive_usize(value, "--max-items")?;
                }
                _ => unreachable!(),
            }
            index += 2;
        } else if option.starts_with('-') {
            return Err((2, format!("unknown option `{option}`")));
        } else if target.replace(args[index].clone()).is_some() {
            return Err((2, "deps requires exactly one file target".into()));
        } else {
            index += 1;
        }
    }
    let target = target.ok_or_else(|| (2, "deps requires exactly one file target".into()))?;
    Ok(DependencyOptions {
        target,
        root,
        direction,
        depth,
        max_items,
    })
}

fn positive_usize(value: &str, option: &str) -> Result<usize, (i32, String)> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| (2, format!("{option} requires a positive integer")))
}

fn traverse_dependencies(
    edges: &[LocalDependencyEdge],
    start: &Path,
    forward: bool,
    max_depth: usize,
    output: &mut Vec<DependencyTraversal>,
) {
    let direction = if forward { "import" } else { "dependent" };
    let mut frontier = HashSet::from([start.to_path_buf()]);
    let mut visited_nodes = frontier.clone();
    let mut visited_edges = HashSet::new();
    for depth in 1..=max_depth {
        let mut candidates = edges
            .iter()
            .filter_map(|edge| {
                let target = &edge.target;
                let matches = if forward {
                    frontier.contains(&edge.source)
                } else {
                    frontier.contains(target)
                };
                matches.then_some((edge, target))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(edge, target)| (&edge.source, *target, edge.line));
        let mut next = HashSet::new();
        for (edge, target) in candidates {
            let key = (edge.source.clone(), target.clone(), edge.line);
            if visited_edges.insert(key) {
                output.push(DependencyTraversal {
                    depth,
                    direction,
                    source: edge.source.clone(),
                    target: target.clone(),
                    line: edge.line,
                });
            }
            let adjacent = if forward { target } else { &edge.source };
            if visited_nodes.insert(adjacent.clone()) {
                next.insert(adjacent.clone());
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
}

fn language_for(path: &Path, explicit: Option<Language>) -> Result<Language, (i32, String)> {
    let detected = Language::infer(path);
    match (explicit, detected) {
        (Some(explicit), Ok(detected)) if explicit != detected => Err((
            2,
            format!(
                "language mismatch: explicit {} but `{}` is {}",
                explicit.name(),
                path.display(),
                detected.name()
            ),
        )),
        (Some(explicit), _) => Ok(explicit),
        (None, Ok(detected)) => Ok(detected),
        (None, Err(error)) => Err((2, error)),
    }
}

fn render_outline(
    parsed: &ParsedFile,
    cwd: &Path,
    max_items: usize,
    selectors: bool,
    signatures: bool,
    matches: &[String],
    output: &mut dyn Write,
) -> CommandResult {
    let shown_path = display_path(&parsed.path, cwd);
    let exact_matches = matches
        .iter()
        .map(|term| {
            parsed
                .symbols
                .iter()
                .any(|symbol| symbol.qualified_name.to_lowercase() == *term)
        })
        .collect::<Vec<_>>();
    let selected = parsed
        .symbols
        .iter()
        .filter(|symbol| outline_symbol_matches(symbol, matches, &exact_matches))
        .collect::<Vec<_>>();
    let shown = selected.len().min(max_items);
    if matches.is_empty() {
        writeln!(
            output,
            "# pira_codenav outline file={} language={} parse={} symbols={} shown={} omitted={}",
            shown_path,
            parsed.language.name(),
            parsed.state.as_str(),
            parsed.symbols.len(),
            shown,
            selected.len().saturating_sub(shown)
        )
        .map_err(output_error)?;
    } else {
        writeln!(
            output,
            "# pira_codenav outline file={} language={} parse={} symbols={} matched={} shown={} omitted={}",
            shown_path,
            parsed.language.name(),
            parsed.state.as_str(),
            parsed.symbols.len(),
            selected.len(),
            shown,
            selected.len().saturating_sub(shown)
        )
        .map_err(output_error)?;
    }
    for symbol in selected.into_iter().take(shown) {
        let indent = "  ".repeat(symbol.depth);
        write!(
            output,
            "{indent}{} {} L{}:{}-{}:{}",
            symbol.kind,
            symbol.qualified_name,
            symbol.start_row + 1,
            symbol.start_column + 1,
            symbol.end_row + 1,
            symbol.end_column + 1
        )
        .map_err(output_error)?;
        if signatures {
            write!(output, " signature={}", quote_metadata(&symbol.signature))
                .map_err(output_error)?;
        }
        if selectors {
            write!(output, " selector={}", parsed.selector(symbol, &shown_path))
                .map_err(output_error)?;
        }
        writeln!(output).map_err(output_error)?;
    }
    Ok(())
}

fn outline_symbol_matches(symbol: &Symbol, matches: &[String], exact_matches: &[bool]) -> bool {
    if matches.is_empty() {
        return true;
    }
    let name = symbol.qualified_name.to_lowercase();
    let signature = symbol.signature.to_lowercase();
    matches.iter().zip(exact_matches).any(|(term, exact)| {
        if *exact {
            name == *term
        } else {
            symbol.kind.contains(term) || name.contains(term) || signature.contains(term)
        }
    })
}

fn render_source(
    parsed: &ParsedFile,
    symbol: &Symbol,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    let source = parsed
        .source
        .get(symbol.start_byte..symbol.end_byte)
        .unwrap_or_default();
    writeln!(
        output,
        "# pira_codenav show file={} language={} item={} kind={} range=L{}:{}-{}:{} bytes={} hash={}",
        display_path(&parsed.path, cwd),
        parsed.language.name(),
        symbol.qualified_name,
        symbol.kind,
        symbol.start_row + 1,
        symbol.start_column + 1,
        symbol.end_row + 1,
        symbol.end_column + 1,
        source.len(),
        hash16(source.as_bytes())
    )
    .map_err(output_error)?;
    let (rendered, escaped_controls) = render_untrusted_source(source);
    render_source_boundary(output, escaped_controls)?;
    write!(output, "{rendered}").map_err(output_error)?;
    if !rendered.ends_with('\n') {
        writeln!(output).map_err(output_error)?;
    }
    writeln!(output, "--- end source ---").map_err(output_error)?;
    Ok(())
}

fn render_line_range(
    path: &Path,
    language: Language,
    start: usize,
    requested_end: usize,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    if start == 0 || requested_end < start {
        return Err((2, "line range must satisfy 1 <= START <= END".into()));
    }
    let source = read_source(path).map_err(input_error)?;
    let mut starts = vec![0];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' && index + 1 < source.len() {
            starts.push(index + 1);
        }
    }
    let line_count = usize::from(!source.is_empty()) * starts.len();
    if start > line_count {
        return Err((
            2,
            format!(
                "line range starts at {start}, beyond {} line(s) in {}",
                line_count,
                path.display()
            ),
        ));
    }
    let end = requested_end.min(line_count);
    let start_byte = starts[start - 1];
    let end_byte = starts.get(end).copied().unwrap_or(source.len());
    let selected = &source[start_byte..end_byte];
    let final_line = selected
        .trim_end_matches(['\n', '\r'])
        .rsplit_once('\n')
        .map_or_else(
            || selected.trim_end_matches(['\n', '\r']),
            |(_, line)| line.trim_end_matches('\r'),
        );
    writeln!(
        output,
        "# pira_codenav show file={} language={} item=lines:{}-{} kind=range range=L{}:1-{}:{} bytes={} hash={}",
        display_path(path, cwd),
        language.name(),
        start,
        end,
        start,
        end,
        final_line.len() + 1,
        selected.len(),
        hash16(selected.as_bytes())
    )
    .map_err(output_error)?;
    let (rendered, escaped_controls) = render_untrusted_source(selected);
    render_source_boundary(output, escaped_controls)?;
    write!(output, "{rendered}").map_err(output_error)?;
    if !rendered.ends_with('\n') {
        writeln!(output).map_err(output_error)?;
    }
    writeln!(output, "--- end source ---").map_err(output_error)?;
    Ok(())
}

fn render_source_boundary(output: &mut dyn Write, escaped_controls: usize) -> CommandResult {
    if escaped_controls == 0 {
        writeln!(output, "--- begin untrusted repository source ---").map_err(output_error)
    } else {
        writeln!(
            output,
            "--- begin untrusted repository source controls_escaped={escaped_controls} ---"
        )
        .map_err(output_error)
    }
}

fn render_untrusted_source(source: &str) -> (String, usize) {
    let mut escaped = 0;
    let mut output = String::with_capacity(source.len());
    for character in source.chars() {
        if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
            use std::fmt::Write as _;
            let _ = write!(output, "\\u{{{:x}}}", character as u32);
            escaped += 1;
        } else {
            output.push(character);
        }
    }
    (output, escaped)
}

fn warn_partial(parsed: &ParsedFile) {
    match parsed.state {
        crate::model::ParseState::Ok => {}
        crate::model::ParseState::Recovered if parsed.original_defects == 0 => eprintln!(
            "warning: parse recovered for {}: remaining parser gaps are confined to recognized function bodies",
            parsed.path.display()
        ),
        crate::model::ParseState::Recovered => eprintln!(
            "warning: parse recovered for {}: resolved {} navigation-relevant ERROR/MISSING node(s) ({} raw)",
            parsed.path.display(),
            parsed.original_defects,
            parsed.raw_defects
        ),
        crate::model::ParseState::Partial if parsed.defects < parsed.original_defects => eprintln!(
            "warning: parse is partial for {}: {} ERROR/MISSING node(s) remain after macro recovery (originally {})",
            parsed.path.display(),
            parsed.defects,
            parsed.original_defects
        ),
        crate::model::ParseState::Partial => eprintln!(
            "warning: parse is partial for {}: {} ERROR/MISSING node(s)",
            parsed.path.display(),
            parsed.defects
        ),
    }
}

struct FileDiscovery {
    files: Vec<(PathBuf, Language)>,
    discovered: usize,
    unsupported: usize,
    ambiguous: usize,
}

fn discover_files(root: &Path, filter: Option<Language>) -> FileDiscovery {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .require_git(false)
        .follow_links(false);
    let mut files = Vec::new();
    let mut discovered = 0;
    let mut unsupported = 0;
    let mut ambiguous = 0;
    for entry in builder.build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        discovered += 1;
        let path = absolute_lexical(entry.path(), root);
        if let Some(language) = filter {
            if language.matches_path(&path) {
                files.push((path, language));
            } else {
                unsupported += 1;
            }
        } else if Language::is_ambiguous_path(&path) {
            ambiguous += 1;
        } else if let Ok(language) = Language::infer(&path) {
            files.push((path, language));
        } else {
            unsupported += 1;
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    FileDiscovery {
        files,
        discovered,
        unsupported,
        ambiguous,
    }
}

fn parse_paths_and_limit(
    args: &[String],
    allow_selectors: bool,
) -> Result<(Vec<String>, usize, bool), (i32, String)> {
    let mut paths = Vec::new();
    let mut max_items = DEFAULT_MAX_ITEMS;
    let mut selectors = false;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--max-items" {
            let Some(value) = args.get(index + 1) else {
                return Err((2, "--max-items requires a positive integer".into()));
            };
            max_items = value
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| (2, "--max-items requires a positive integer".into()))?;
            index += 2;
        } else if args[index] == "--selectors" && allow_selectors {
            selectors = true;
            index += 1;
        } else if args[index].starts_with('-') {
            return Err((2, format!("unknown option `{}`", args[index])));
        } else {
            paths.push(args[index].clone());
            index += 1;
        }
    }
    Ok((paths, max_items, selectors))
}

#[derive(Debug)]
struct DecodedSelector {
    language: Language,
    path: String,
    kind: String,
    qualified: String,
    hash: String,
}

fn parse_selector(value: &str) -> Result<DecodedSelector, String> {
    let rest = value
        .strip_prefix("pira://")
        .ok_or_else(|| "selector must begin with pira://".to_string())?;
    let (language, rest) = rest
        .split_once('/')
        .ok_or_else(|| "selector is missing language/path separator".to_string())?;
    let language = Language::parse_name(language)
        .ok_or_else(|| format!("unsupported selector language `{language}`"))?;
    let (path, rest) = rest
        .split_once('#')
        .ok_or_else(|| "selector is missing symbol fragment".to_string())?;
    let (kind, rest) = rest
        .split_once('/')
        .ok_or_else(|| "selector is missing kind/name separator".to_string())?;
    let (qualified, hash) = rest
        .rsplit_once('@')
        .ok_or_else(|| "selector is missing source hash".to_string())?;
    if hash.len() != 16 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("selector source hash is invalid".into());
    }
    Ok(DecodedSelector {
        language,
        path: percent_decode(path)?,
        kind: percent_decode(kind)?,
        qualified: percent_decode(qualified)?,
        hash: hash.to_ascii_lowercase(),
    })
}

fn target_path(target: &str, cwd: &Path) -> Option<PathBuf> {
    if let Some((path, _, _)) = parse_location(target) {
        return Some(absolute_lexical(Path::new(path), cwd));
    }
    split_existing_symbol_target(target, cwd).map(|(path, _)| path)
}

fn parse_location(value: &str) -> Option<(&str, usize, Option<usize>)> {
    let (prefix, last) = value.rsplit_once(':')?;
    let last_number = last.parse::<usize>().ok()?;
    if let Some((path, line)) = prefix.rsplit_once(':')
        && let Ok(line) = line.parse::<usize>()
    {
        return Some((path, line, Some(last_number)));
    }
    Some((prefix, last_number, None))
}

fn parse_line_range(value: &str) -> Option<(&str, usize, usize)> {
    let (path, range) = value.rsplit_once(':')?;
    let (start, end) = range.split_once('-')?;
    Some((path, start.parse().ok()?, end.parse().ok()?))
}

fn split_existing_symbol_target(target: &str, cwd: &Path) -> Option<(PathBuf, String)> {
    for (index, _) in target.match_indices("::") {
        let path = absolute_lexical(Path::new(&target[..index]), cwd);
        if path.is_file() {
            return Some((path, target[index + 2..].to_owned()));
        }
    }
    None
}

fn print_global_help(output: &mut dyn Write) -> CommandResult {
    writeln!(
        output,
        "pira_codenav {VERSION} — read-only structural source navigation\n\nUSAGE\n  pira_codenav [LANGUAGE] <SUBCOMMAND> [ARGS...]\n\nSUBCOMMANDS\n  outline FILE...       compact declarations; use --match to filter\n  show TARGET...        exact source for bounded outlined items or locations\n  map DIRECTORY         bounded mixed-language repository shape\n  imports FILE...       import/include statements and structural targets\n  dependents FILE       direct reverse file/module dependencies\n  deps FILE             bounded transitive local file dependencies\n  languages             installed language capabilities\n\nTYPICAL FLOW\n  Start with `map DIRECTORY --max-items 200`, outline a relevant file with --match, then show only\n  the needed item or line span. Use imports/dependents/deps only when file relationships matter.\n\nLANGUAGE\n  Normally inferred from path suffix or shebang. Supported: python, rust, java, c, cpp, cuda, bash,\n  go, javascript, typescript, csharp, powershell, php, kotlin, lua, hcl, r. Use an explicit language\n  for extensionless or ambiguous .h files, or to filter directory operations. TypeScript includes\n  TSX; HCL includes Terraform files.\n\nBOUNDARY\n  This tool is read-only and structural. Use a language server or compiler for definitions,\n  references, types, calls, diagnostics, or macro/build semantics.\n\nRun `pira_codenav <SUBCOMMAND> --help` or `pira_codenav <LANGUAGE> --help` for details."
    )
    .map_err(output_error)
}

fn print_language_help(language: Language, output: &mut dyn Write) -> CommandResult {
    writeln!(
        output,
        "pira_codenav {} — read-only structural operations\n\nUSAGE\n  pira_codenav {} <outline|show|map|imports|dependents|deps> [ARGS...]\n\nAn explicit language parses extensionless files and filters directory operations. A conflicting recognized suffix is an error.",
        language.name(),
        language.name()
    )
    .map_err(output_error)
}

const OUTLINE_HELP: &str = r#"pira_codenav outline — inspect declarations without implementation bodies

WHEN TO USE
  Use after map, or when the relevant file is already known. Use --match before reading source.

USAGE
  pira_codenav [LANGUAGE] outline FILE... [--match TEXT]... [--max-items N]
    [--signatures] [--selectors]

OUTPUT AND FILTERING
  Prints parse completeness, nested declaration kinds/names, and exact source ranges. Bodies and
  signatures are omitted by default. --signatures adds overload/type detail. Repeated --match values
  are case-insensitive OR filters over kind, qualified name, and signature; they are not regexes.
  Exact qualified-name matches take precedence over broader substring matches.

BOUNDS AND HANDOFF
  The default is 1,000 items per file. Use FILE:LINE with show for the compact normal handoff. Add
  --selectors only when a freshness-checked identity must survive edits or later turns. Multiple-file
  errors do not discard successful outlines."#;

const SHOW_HELP: &str = r#"pira_codenav show — retrieve one exact structural item or line span

WHEN TO USE
  Use after outline identifies a relevant range. Prefer the smallest sufficient item or line span.

USAGE
  pira_codenav [LANGUAGE] show TARGET... [--max-items N] [--max-bytes N]

TARGETS
  TARGET is an outline selector, FILE:LINE[:COLUMN], FILE::QUALIFIED-NAME, or—only for a single
  target—FILE:START-END. A location selects the smallest enclosing named item. A line span is exact,
  inclusive, and stops at EOF when END is larger. There is no --symbol option.

BOUNDS AND OUTPUT
  One target without --max-bytes returns the whole selected item; use a line span for an unusually
  large item. Multi-target output is deduplicated and defaults to 20 whole items and 64 KiB. Limits
  omit whole items rather than truncating source. Selectors reject stale source. Source is framed as
  untrusted repository data; printable text is preserved, while unsafe control characters are escaped
  and reported in the boundary metadata."#;

const MAP_HELP: &str = r#"pira_codenav map — produce a bounded repository or subsystem shape

WHEN TO USE
  Start here when the relevant files are unknown.

USAGE
  pira_codenav [LANGUAGE] map DIRECTORY [--max-items N]

OUTPUT AND BOUNDS
  Prints one accounting header and compact file rows with language, parse state, and representative
  top-level names. The default ceiling is 1,000 files; start with --max-items 200 or a narrower
  directory for broad repositories. Selection is deterministic and balanced across parent directories.
  Without LANGUAGE, each supported file is detected independently; an explicit language filters the
  walk. Git ignore rules are honored and symlinked directories are not followed."#;

const IMPORTS_HELP: &str = r#"pira_codenav imports — inspect direct import/include statements

WHEN TO USE
  Use when a file's immediate structural dependencies matter. Run from the intended workspace root,
  because conservative local target resolution is rooted at the current directory.

USAGE
  pira_codenav [LANGUAGE] imports FILE...

OUTPUT
  Prints exact import text, source line, resolution status, and a structurally resolved local target
  when available. External, dynamic, ambiguous, package-dependent, and build-dependent targets remain
  visibly unresolved. Multiple-file errors do not discard successful results.
  Never invokes a package manager or build system, and never executes a source file."#;

const DEPENDENTS_HELP: &str = r#"pira_codenav dependents — inspect direct reverse file dependencies

WHEN TO USE
  Use to find files whose imports structurally resolve to one known file. Narrow --root when possible.

USAGE
  pira_codenav [LANGUAGE] dependents FILE [--root DIRECTORY]

OUTPUT AND SCOPE
  FILE is relative to --root, which defaults to the current directory. The command scans inferred
  files of the target language under that root and prints every direct matching import with source
  line and resolution. This is conservative file navigation, not compiler-semantic reference search."#;

const DEPS_HELP: &str = r#"pira_codenav deps — traverse bounded local structural file dependencies

WHEN TO USE
  Use after imports or dependents when bounded transitive file relationships are needed.

USAGE
  pira_codenav [LANGUAGE] deps FILE [--direction imports|dependents|both] [--depth N]
    [--root DIRECTORY] [--max-items N]

OUTPUT AND BOUNDS
  FILE is a path relative to --root, not FILE:LINE or a symbol target. The command builds an in-memory
  same-language graph from conservative local import targets and prints bounded depth/direction edges.
  Defaults: direction=both, depth=2, root=current directory, max-items=1,000. Pass a lower --max-items
  or narrower --root for broad repositories. It does not infer symbol references, calls, build-system
  edges, package resolution, or dynamic imports."#;

const LANGUAGES_HELP: &str = r#"pira_codenav languages — list installed language capabilities

WHEN TO USE
  Use when language inference or support is uncertain.

USAGE
  pira_codenav languages

OUTPUT
  Prints one shared native-parser capability header followed by one supported language name per line."#;

fn print_command_help(command: &str, output: &mut dyn Write) -> CommandResult {
    let text = match command {
        "outline" => OUTLINE_HELP,
        "show" => SHOW_HELP,
        "map" => MAP_HELP,
        "imports" => IMPORTS_HELP,
        "dependents" => DEPENDENTS_HELP,
        "deps" => DEPS_HELP,
        "languages" => LANGUAGES_HELP,
        other => {
            return Err((
                2,
                format!("unknown subcommand `{other}`; run pira_codenav --help"),
            ));
        }
    };
    writeln!(output, "{text}").map_err(output_error)
}

fn print_languages(output: &mut dyn Write) -> CommandResult {
    writeln!(
        output,
        "# pira_codenav languages count={} parser=native capabilities=outline,show,map,imports,dependents,deps",
        Language::ALL.len()
    )
    .map_err(output_error)?;
    for language in Language::ALL {
        writeln!(output, "{}", language.name()).map_err(output_error)?;
    }
    Ok(())
}

fn usage<T: Into<String>>(message: T) -> CommandResult {
    Err((2, message.into()))
}

fn input_error<T: Into<String>>(message: T) -> (i32, String) {
    (2, message.into())
}

fn output_error(error: io::Error) -> (i32, String) {
    if error.kind() == io::ErrorKind::BrokenPipe {
        (0, String::new())
    } else {
        (1, format!("cannot write output: {error}"))
    }
}

fn finish_output(result: CommandResult, output: &mut dyn Write) -> i32 {
    let result = result.and_then(|()| output.flush().map_err(output_error));
    match result {
        Ok(()) | Err((0, _)) => 0,
        Err((code, message)) => fail(code, message),
    }
}

fn fail<T: AsRef<str>>(code: i32, message: T) -> i32 {
    eprintln!("error: {}", message.as_ref());
    code
}

#[cfg(test)]
mod tests {
    use super::{parse_location, parse_selector, render_untrusted_source};
    use crate::language::Language;

    #[test]
    fn selector_decodes_unicode_and_delimiters() {
        let selector = parse_selector(
            "pira://rust/src%2Fna%C3%AFve.rs#method/Parser%3A%3Aparse@0123456789abcdef",
        )
        .expect("valid selector");
        assert_eq!(selector.language, Language::Rust);
        assert_eq!(selector.path, "src/naïve.rs");
        assert_eq!(selector.kind, "method");
        assert_eq!(selector.qualified, "Parser::parse");
    }

    #[test]
    fn location_parser_preserves_windows_drive_prefix() {
        assert_eq!(
            parse_location(r"C:\repo\src\lib.rs:42:7"),
            Some((r"C:\repo\src\lib.rs", 42, Some(7)))
        );
        assert_eq!(
            parse_location("src/lib.rs:42"),
            Some(("src/lib.rs", 42, None))
        );
    }

    #[test]
    fn source_renderer_escapes_only_dangerous_controls() {
        let (rendered, count) = render_untrusted_source("a\tb\n\u{1b}c\0");
        assert_eq!(rendered, "a\tb\n\\u{1b}c\\u{0}");
        assert_eq!(count, 2);
    }
}
