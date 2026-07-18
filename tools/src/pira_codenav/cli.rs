use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::ffi::OsString;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rayon::prelude::*;
use regex::{Regex, RegexBuilder};

use crate::command::{
    CommandResult, input_error, language_for, lsp_error, output_error, parse_location,
    positive_usize,
};
use crate::deps;
use crate::language::Language;
use crate::lsp_options::{self, LspOptions};
use crate::model::{ImportEdge, ParseBackend, Symbol};
use crate::parse::{ParsedFile, parse_file, parse_syntax};
use crate::semantic;
use crate::structural::StructuralResolver;
use crate::util::{
    DEFAULT_MAX_ITEMS, absolute_lexical, display_path, escape_untrusted_text, hash16,
    percent_decode, quote_metadata, read_source, sanitize_metadata,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_SHOW_MAX_ITEMS: usize = 20;
const DEFAULT_SHOW_MAX_BYTES: usize = 64 * 1024;
const MAX_REPORTED_FAILURES: usize = 20;
const MAX_FAILURE_SUBJECT_BYTES: usize = 512;
const MAX_FAILURE_MESSAGE_BYTES: usize = 2 * 1024;
const PARSE_BATCH_FILES: usize = 16;
const MAX_FIND_ITEMS: usize = 100_000;

type ParsedFileCache = BTreeMap<(PathBuf, Language), Result<ParsedFile, (i32, String)>>;

struct CommandFailure {
    subject: String,
    code: i32,
    message: String,
}

#[derive(Default)]
struct FailureCollector {
    total: usize,
    first_code: Option<i32>,
    shown: Vec<CommandFailure>,
}

impl FailureCollector {
    fn record(&mut self, subject: String, code: i32, message: String) {
        debug_assert!(code >= 2, "output errors must not become target failures");
        self.total += 1;
        self.first_code.get_or_insert(code);
        if self.shown.len() < MAX_REPORTED_FAILURES {
            self.shown.push(CommandFailure {
                subject: bounded_metadata(&subject, MAX_FAILURE_SUBJECT_BYTES),
                code,
                message: bounded_metadata(&message, MAX_FAILURE_MESSAGE_BYTES),
            });
        }
    }

    fn complete(&self) -> usize {
        usize::from(self.total == 0)
    }

    fn omitted(&self) -> usize {
        self.total.saturating_sub(self.shown.len())
    }

    fn first_code(&self) -> i32 {
        self.first_code
            .expect("a non-empty failure collector has a first code")
    }

    fn merge_sorted(mut self, other: Self) -> Self {
        self.total += other.total;
        if self.first_code.is_none() {
            self.first_code = other.first_code;
        }
        self.shown.extend(other.shown);
        self.shown.sort_by(|left, right| {
            left.subject
                .cmp(&right.subject)
                .then_with(|| left.code.cmp(&right.code))
                .then_with(|| left.message.cmp(&right.message))
        });
        self.shown.truncate(MAX_REPORTED_FAILURES);
        self
    }
}

fn bounded_metadata(value: &str, max_bytes: usize) -> String {
    let mut value = sanitize_metadata(value);
    if value.len() > max_bytes {
        let mut end = max_bytes.saturating_sub('…'.len_utf8());
        while !value.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        value.truncate(end);
        value.push('…');
    }
    value
}

fn finish_partial_result(
    attempted: usize,
    successful: usize,
    failures: &FailureCollector,
    all_failed_message: &str,
    output: &mut dyn Write,
) -> CommandResult {
    if attempted > 0 && successful == 0 && failures.total > 0 {
        output.flush().map_err(output_error)?;
        return Err((failures.first_code(), all_failed_message.into()));
    }
    Ok(())
}

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
    let (values, lsp) = match lsp_options::parse(&values, &command, &cwd) {
        Ok(parsed) => parsed,
        Err(error) => return fail(error.0, error.1),
    };
    let result = match command.as_str() {
        "outline" => command_outline(&values, explicit_language, &cwd, &lsp, &mut output),
        "show" => command_show(&values, explicit_language, &cwd, &lsp, &mut output),
        "map" => command_map(&values, explicit_language, &cwd, &lsp, &mut output),
        "find" => command_find(&values, explicit_language, &cwd, &lsp, &mut output),
        "definition" => semantic::definition(&values, explicit_language, &cwd, &lsp, &mut output),
        "implementation" => {
            semantic::implementation(&values, explicit_language, &cwd, &lsp, &mut output)
        }
        "type-definition" => {
            semantic::type_definition(&values, explicit_language, &cwd, &lsp, &mut output)
        }
        "references" => semantic::references(&values, explicit_language, &cwd, &lsp, &mut output),
        "hover" => semantic::hover(&values, explicit_language, &cwd, &lsp, &mut output),
        "callers" => semantic::callers(&values, explicit_language, &cwd, &lsp, &mut output),
        "callees" => semantic::callees(&values, explicit_language, &cwd, &lsp, &mut output),
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

fn command_outline(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    let options = parse_outline_options(args)?;
    if options.paths.is_empty() {
        return usage("outline requires at least one file");
    }
    let total = options.paths.len();
    let mut failures = FailureCollector::default();
    let mut resolver = StructuralResolver::new(lsp.config(cwd)?);
    for path in options.paths {
        let absolute = absolute_lexical(Path::new(&path), cwd);
        let result = (|| {
            let language = language_for(&absolute, explicit)?;
            let parsed = resolver
                .resolve(parse_file(&absolute, language).map_err(input_error)?)
                .map_err(lsp_error)?;
            render_outline(
                &parsed,
                cwd,
                options.max_items,
                options.selectors,
                options.signatures,
                &options.matches,
                output,
            )?;
            Ok(())
        })();
        if let Err((code, message)) = result {
            if total == 1 || code <= 1 {
                return Err((code, message));
            }
            failures.record(path, code, message);
        }
    }
    for failure in &failures.shown {
        writeln!(
            output,
            "# pira_codenav outline error file={} code={} message={}",
            quote_metadata(&failure.subject),
            failure.code,
            quote_metadata(&failure.message)
        )
        .map_err(output_error)?;
    }
    if total > 1 {
        writeln!(
            output,
            "# pira_codenav outline batch files={} succeeded={} failed={} complete={} errors_shown={} errors_omitted={}",
            total,
            total.saturating_sub(failures.total),
            failures.total,
            failures.complete(),
            failures.shown.len(),
            failures.omitted()
        )
        .map_err(output_error)?;
    }
    finish_partial_result(
        total,
        total.saturating_sub(failures.total),
        &failures,
        "all outline files failed; inspect the reported file errors",
        output,
    )
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
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    let options = parse_show_options(args)?;
    let mut parsed_files = ParsedFileCache::new();
    let mut resolver = StructuralResolver::new(lsp.config(cwd)?);
    if options.targets.len() == 1
        && let Some((path_text, start, end)) = parse_line_range(&options.targets[0])
    {
        let path = absolute_lexical(Path::new(path_text), cwd);
        language_for(&path, explicit)?;
        let mut item = Vec::new();
        render_line_range(&path, start, end, cwd, &mut item)?;
        if let Some(max_bytes) = options.max_bytes
            && item.len() > max_bytes
        {
            writeln!(
                output,
                "# pira_codenav show targets=1 shown=0 omitted=1 byte_limited=1 max_bytes={}",
                max_bytes
            )
            .map_err(output_error)?;
            return Ok(());
        }
        output.write_all(&item).map_err(output_error)?;
        return Ok(());
    }
    if options.targets.len() == 1 && options.max_bytes.is_none() {
        let (key, symbol_index) = resolve_show_target(
            &options.targets[0],
            explicit,
            cwd,
            &mut parsed_files,
            &mut resolver,
        )?;
        let parsed = parsed_files
            .get(&key)
            .and_then(|result| result.as_ref().ok())
            .expect("resolved show target has a cached parse");
        render_source(parsed, &parsed.symbols[symbol_index], cwd, output)?;
        return Ok(());
    }

    let max_items = options.max_items.unwrap_or(DEFAULT_SHOW_MAX_ITEMS);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_SHOW_MAX_BYTES);
    let mut rendered = Vec::new();
    let mut identities = HashSet::new();
    let mut duplicates = 0;
    let mut byte_limited = 0;
    let mut failures = FailureCollector::default();
    let mut resolved = 0;
    let mut considered = 0;
    let mut payload_bytes = 0;
    for target in &options.targets {
        if considered >= max_items {
            break;
        }
        let (key, symbol_index) =
            match resolve_show_target(target, explicit, cwd, &mut parsed_files, &mut resolver) {
                Ok(resolved) => resolved,
                Err((code, message)) => {
                    failures.record(target.clone(), code, message);
                    continue;
                }
            };
        resolved += 1;
        let parsed = parsed_files
            .get(&key)
            .and_then(|result| result.as_ref().ok())
            .expect("resolved show target has a cached parse");
        let symbol = &parsed.symbols[symbol_index];
        let identity = (parsed.path.clone(), symbol.start_byte, symbol.end_byte);
        if !identities.insert(identity) {
            duplicates += 1;
            continue;
        }
        considered += 1;
        let mut item = Vec::new();
        render_source(parsed, symbol, cwd, &mut item)?;
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
        .saturating_sub(rendered.len() + duplicates + failures.total);
    write!(
        output,
        "# pira_codenav show targets={} shown={}",
        options.targets.len(),
        rendered.len()
    )
    .map_err(output_error)?;
    if failures.total > 0 {
        write!(output, " failed={} complete=0", failures.total).map_err(output_error)?;
    }
    if duplicates > 0 {
        write!(output, " duplicates={duplicates}").map_err(output_error)?;
    }
    if omitted > 0 {
        write!(output, " omitted={omitted}").map_err(output_error)?;
    }
    if byte_limited > 0 {
        write!(output, " byte_limited={byte_limited} max_bytes={max_bytes}")
            .map_err(output_error)?;
    }
    if failures.omitted() > 0 {
        write!(output, " errors_omitted={}", failures.omitted()).map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    for failure in &failures.shown {
        writeln!(
            output,
            "error target={} code={} message={}",
            quote_metadata(&failure.subject),
            failure.code,
            quote_metadata(&failure.message)
        )
        .map_err(output_error)?;
    }
    for item in rendered {
        output.write_all(&item).map_err(output_error)?;
    }
    finish_partial_result(
        options.targets.len(),
        resolved,
        &failures,
        "all show targets failed; inspect the reported target errors",
        output,
    )
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
    parsed_files: &mut ParsedFileCache,
    resolver: &mut StructuralResolver,
) -> Result<((PathBuf, Language), usize), (i32, String)> {
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
    let key = (path, language);
    if !parsed_files.contains_key(&key) {
        let parsed = parse_file(&key.0, key.1)
            .map_err(input_error)
            .and_then(|parsed| resolver.resolve(parsed).map_err(lsp_error));
        parsed_files.insert(key.clone(), parsed);
    }
    let parsed = match parsed_files
        .get(&key)
        .expect("show parse was inserted before target resolution")
    {
        Ok(parsed) => parsed,
        Err(error) => return Err(error.clone()),
    };
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
    Ok((key, symbol_index))
}

fn qualified_suffix_matches(candidate: &str, query: &str) -> bool {
    candidate == query
        || candidate.strip_suffix(query).is_some_and(|prefix| {
            prefix.ends_with('.') || prefix.ends_with("::") || prefix.ends_with('\\')
        })
}

fn command_map(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
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
    let discovery = discover_files(
        &root,
        explicit.map_or(DiscoverySelection::Any, DiscoverySelection::Exact),
    );
    let mut failures = FailureCollector::default();
    let mut summaries = Vec::with_capacity(discovery.files.len());
    let mut resolver = StructuralResolver::new(lsp.config(&root)?);
    for batch in discovery.files.chunks(PARSE_BATCH_FILES) {
        let parsed = batch
            .par_iter()
            .map(|(path, language)| parse_file(path, *language))
            .collect::<Vec<_>>();
        for ((path, _), result) in batch.iter().zip(parsed) {
            match result {
                Ok(parsed) => match resolver.resolve(parsed) {
                    Ok(parsed) => summaries.push(FileSummary {
                        path: parsed.path,
                        language: parsed.language,
                        backend: parsed.backend,
                        names: top_level_map_names(&parsed.symbols),
                    }),
                    Err(message) => failures.record(display_path(path, &root), 3, message),
                },
                Err(message) => failures.record(display_path(path, &root), 2, message),
            }
        }
    }
    let parsed_count = summaries.len();
    let lsp_count = summaries
        .iter()
        .filter(|summary| summary.backend == ParseBackend::Lsp)
        .count();
    let shown_summaries = balanced_summaries(summaries, &root, max_items);
    let shown = shown_summaries.len();
    write!(
        output,
        "# pira_codenav map root={} files={} shown={}",
        display_path(&root, cwd),
        discovery.files.len(),
        shown
    )
    .map_err(output_error)?;
    if parsed_count != discovery.files.len() {
        write!(
            output,
            " parsed={} failed={} complete=0",
            parsed_count, failures.total
        )
        .map_err(output_error)?;
    }
    if lsp_count > 0 {
        write!(output, " lsp={lsp_count}").map_err(output_error)?;
    }
    if discovery.unsupported > 0 {
        write!(output, " unsupported={}", discovery.unsupported).map_err(output_error)?;
    }
    if discovery.ambiguous > 0 {
        write!(output, " ambiguous={}", discovery.ambiguous).map_err(output_error)?;
    }
    let omitted = parsed_count.saturating_sub(shown);
    if omitted > 0 {
        write!(output, " omitted={omitted}").map_err(output_error)?;
    }
    if failures.omitted() > 0 {
        write!(output, " errors_omitted={}", failures.omitted()).map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    for failure in &failures.shown {
        writeln!(
            output,
            "error file={} code={} message={}",
            quote_metadata(&failure.subject),
            failure.code,
            quote_metadata(&failure.message)
        )
        .map_err(output_error)?;
    }
    for file in shown_summaries {
        let names = file.names.join(",");
        write!(
            output,
            "file={} language={}",
            display_path(&file.path, cwd),
            file.language.name()
        )
        .map_err(output_error)?;
        if file.backend == ParseBackend::Lsp {
            write!(output, " backend=lsp").map_err(output_error)?;
        }
        writeln!(output, " symbols={names}").map_err(output_error)?;
    }
    finish_partial_result(
        discovery.files.len(),
        parsed_count,
        &failures,
        "all eligible map files failed; inspect the reported file errors",
        output,
    )
}

enum FindMatcher {
    Literal(Regex),
    Exact(Regex),
    Regex(Regex),
}

impl FindMatcher {
    fn matches(&self, symbol: &Symbol) -> bool {
        match self {
            Self::Literal(regex) | Self::Regex(regex) => {
                regex.is_match(&symbol.qualified_name)
                    || regex.is_match(&symbol.signature)
                    || regex.is_match(symbol.kind)
            }
            Self::Exact(regex) => regex.is_match(&symbol.qualified_name),
        }
    }

    const fn name(&self) -> &'static str {
        match self {
            Self::Literal(_) => "literal",
            Self::Exact(_) => "exact",
            Self::Regex(_) => "regex",
        }
    }
}

struct FindOptions {
    root: String,
    query: String,
    matcher: FindMatcher,
    kind: Option<String>,
    max_items: usize,
    selectors: bool,
    signatures: bool,
}

struct FindRow {
    path: PathBuf,
    language: Language,
    backend: ParseBackend,
    symbol: Symbol,
    selector: Option<String>,
}

fn command_find(
    args: &[String],
    explicit: Option<Language>,
    cwd: &Path,
    lsp: &LspOptions,
    output: &mut dyn Write,
) -> CommandResult {
    let options = parse_find_options(args)?;
    let root = absolute_lexical(Path::new(&options.root), cwd);
    if !root.is_dir() {
        return Err(input_error(format!(
            "find target is not a directory: {}",
            root.display()
        )));
    }
    let discovery = discover_files(
        &root,
        explicit.map_or(DiscoverySelection::Any, DiscoverySelection::Exact),
    );
    let mut resolver = StructuralResolver::new(lsp.config(&root)?);
    let mut failures = FailureCollector::default();
    let mut parsed_count = 0usize;
    let mut lsp_count = 0usize;
    let mut matched = 0usize;
    let mut rows = Vec::with_capacity(options.max_items.min(1_000));
    for batch in discovery.files.chunks(PARSE_BATCH_FILES) {
        let parsed = batch
            .par_iter()
            .map(|(path, language)| parse_file(path, *language))
            .collect::<Vec<_>>();
        for ((path, _), result) in batch.iter().zip(parsed) {
            let parsed = match result {
                Ok(parsed) => match resolver.resolve(parsed) {
                    Ok(parsed) => parsed,
                    Err(message) => {
                        failures.record(display_path(path, &root), 3, message);
                        continue;
                    }
                },
                Err(message) => {
                    failures.record(display_path(path, &root), 2, message);
                    continue;
                }
            };
            parsed_count += 1;
            if parsed.backend == ParseBackend::Lsp {
                lsp_count += 1;
            }
            let shown_path = display_path(&parsed.path, cwd);
            for symbol in &parsed.symbols {
                if options
                    .kind
                    .as_ref()
                    .is_some_and(|kind| !symbol.kind.eq_ignore_ascii_case(kind))
                    || !options.matcher.matches(symbol)
                {
                    continue;
                }
                matched += 1;
                if rows.len() < options.max_items {
                    rows.push(FindRow {
                        path: parsed.path.clone(),
                        language: parsed.language,
                        backend: parsed.backend,
                        symbol: symbol.clone(),
                        selector: options
                            .selectors
                            .then(|| parsed.selector(symbol, &shown_path)),
                    });
                }
            }
        }
    }
    write!(
        output,
        "# pira_codenav find root={} query={} mode={} files={} matches={} shown={}",
        display_path(&root, cwd),
        quote_metadata(&options.query),
        options.matcher.name(),
        discovery.files.len(),
        matched,
        rows.len()
    )
    .map_err(output_error)?;
    if parsed_count != discovery.files.len() {
        write!(
            output,
            " parsed={} failed={} complete=0",
            parsed_count, failures.total
        )
        .map_err(output_error)?;
    }
    if lsp_count > 0 {
        write!(output, " lsp={lsp_count}").map_err(output_error)?;
    }
    let skipped = discovery.discovered.saturating_sub(discovery.files.len());
    if skipped > 0 {
        write!(output, " skipped={skipped}").map_err(output_error)?;
    }
    let omitted = matched.saturating_sub(rows.len());
    if omitted > 0 {
        write!(output, " omitted={omitted}").map_err(output_error)?;
    }
    if failures.omitted() > 0 {
        write!(output, " errors_omitted={}", failures.omitted()).map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    for failure in &failures.shown {
        writeln!(
            output,
            "error file={} code={} message={}",
            quote_metadata(&failure.subject),
            failure.code,
            quote_metadata(&failure.message)
        )
        .map_err(output_error)?;
    }
    for row in rows {
        write!(
            output,
            "symbol file={} language={} kind={} name={} range=L{}:{}-{}:{}",
            display_path(&row.path, cwd),
            row.language.name(),
            row.symbol.kind,
            quote_metadata(&sanitize_metadata(&row.symbol.qualified_name)),
            row.symbol.start_row + 1,
            row.symbol.start_column + 1,
            row.symbol.end_row + 1,
            row.symbol.end_column + 1
        )
        .map_err(output_error)?;
        if row.backend == ParseBackend::Lsp {
            write!(output, " backend=lsp").map_err(output_error)?;
        }
        if options.signatures {
            write!(
                output,
                " signature={}",
                quote_metadata(&row.symbol.signature)
            )
            .map_err(output_error)?;
        }
        if let Some(selector) = row.selector {
            write!(output, " selector={selector}").map_err(output_error)?;
        }
        writeln!(output).map_err(output_error)?;
    }
    finish_partial_result(
        discovery.files.len(),
        parsed_count,
        &failures,
        "all eligible find files failed; inspect the reported file errors",
        output,
    )
}

fn parse_find_options(args: &[String]) -> Result<FindOptions, (i32, String)> {
    let mut positional = Vec::new();
    let mut exact = false;
    let mut regex = false;
    let mut kind = None;
    let mut max_items = 200usize;
    let mut selectors = false;
    let mut signatures = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--exact" => {
                if exact {
                    return Err((2, "--exact may be specified only once".into()));
                }
                exact = true;
                index += 1;
            }
            "--regex" => {
                if regex {
                    return Err((2, "--regex may be specified only once".into()));
                }
                regex = true;
                index += 1;
            }
            "--kind" => {
                if kind.is_some() {
                    return Err((2, "--kind may be specified only once".into()));
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| (2, "--kind requires a value".into()))?;
                kind = Some(value.to_lowercase());
                index += 2;
            }
            "--max-items" => {
                max_items = positive_usize(
                    args.get(index + 1)
                        .ok_or_else(|| (2, "--max-items requires a value".into()))?,
                    "--max-items",
                )?;
                if max_items > MAX_FIND_ITEMS {
                    return Err((
                        2,
                        format!("find --max-items may not exceed {MAX_FIND_ITEMS}"),
                    ));
                }
                index += 2;
            }
            "--selectors" => {
                selectors = true;
                index += 1;
            }
            "--signatures" => {
                signatures = true;
                index += 1;
            }
            value if value.starts_with('-') => {
                return Err((2, format!("unknown find option `{value}`")));
            }
            value => {
                positional.push(value.to_string());
                index += 1;
            }
        }
    }
    if positional.len() != 2 {
        return Err((2, "find requires DIRECTORY and QUERY".into()));
    }
    if exact && regex {
        return Err((2, "--exact and --regex are mutually exclusive".into()));
    }
    let query = positional.pop().expect("two positional values checked");
    if query.is_empty() || query.len() > 4 * 1024 {
        return Err((2, "find QUERY must contain 1..4096 UTF-8 bytes".into()));
    }
    let root = positional.pop().expect("two positional values checked");
    let matcher = if regex {
        FindMatcher::Regex(build_find_regex(&query, false)?)
    } else if exact {
        let pattern = format!(r"(?:^|\.|::|\\){}$", regex::escape(&query));
        FindMatcher::Exact(build_find_regex(&pattern, true)?)
    } else {
        FindMatcher::Literal(build_find_regex(&regex::escape(&query), true)?)
    };
    Ok(FindOptions {
        root,
        query,
        matcher,
        kind,
        max_items,
        selectors,
        signatures,
    })
}

fn build_find_regex(pattern: &str, case_insensitive: bool) -> Result<Regex, (i32, String)> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .build()
        .map_err(|error| (2, format!("invalid find regex: {error}")))
}

struct FileSummary {
    path: PathBuf,
    language: Language,
    backend: ParseBackend,
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

fn compact_map_name(name: String) -> String {
    const MAX_BYTES: usize = 96;
    bounded_metadata(&name, MAX_BYTES)
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
    let mut failures = FailureCollector::default();
    for value in args {
        let path = absolute_lexical(Path::new(value), cwd);
        let result = (|| {
            let language = language_for(&path, explicit)?;
            let parsed = parse_syntax(&path, language).map_err(input_error)?;
            let edges = deps::imports(&parsed, cwd);
            write!(
                output,
                "# pira_codenav imports file={} count={}",
                display_path(&path, cwd),
                edges.len()
            )
            .map_err(output_error)?;
            if !path_suffix_identifies_language(&path, language) {
                write!(output, " language={}", language.name()).map_err(output_error)?;
            }
            writeln!(output).map_err(output_error)?;
            for edge in edges {
                writeln!(
                    output,
                    "import line={} target={} resolution={} text={}",
                    edge.line,
                    sanitize_metadata(&edge.target_label),
                    edge.resolution,
                    quote_metadata(&edge.text)
                )
                .map_err(output_error)?;
            }
            Ok(())
        })();
        if let Err((code, message)) = result {
            if args.len() == 1 || code <= 1 {
                return Err((code, message));
            }
            failures.record(value.clone(), code, message);
        }
    }
    for failure in &failures.shown {
        writeln!(
            output,
            "# pira_codenav imports error file={} code={} message={}",
            quote_metadata(&failure.subject),
            failure.code,
            quote_metadata(&failure.message)
        )
        .map_err(output_error)?;
    }
    if args.len() > 1 {
        write!(
            output,
            "# pira_codenav imports batch files={} succeeded={}",
            args.len(),
            args.len().saturating_sub(failures.total)
        )
        .map_err(output_error)?;
        if failures.total > 0 {
            write!(output, " failed={} complete=0", failures.total).map_err(output_error)?;
        }
        if failures.omitted() > 0 {
            write!(output, " errors_omitted={}", failures.omitted()).map_err(output_error)?;
        }
        writeln!(output).map_err(output_error)?;
    }
    finish_partial_result(
        args.len(),
        args.len().saturating_sub(failures.total),
        &failures,
        "all imports files failed; inspect the reported file errors",
        output,
    )
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
    let discovery = discover_files(&root, DiscoverySelection::Dependencies(target_language));
    let extracted = extract_dependencies(&discovery.files, &root, Some(&target), |edge| {
        (edge.target.as_deref() == Some(target.as_path())).then_some(edge)
    });
    let mut edges = extracted.edges;
    edges.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then_with(|| left.line.cmp(&right.line))
    });
    write!(
        output,
        "# pira_codenav dependents target={} root={} scanned={} count={}",
        display_path(&target, &root),
        display_path(&root, cwd),
        extracted.scanned,
        edges.len()
    )
    .map_err(output_error)?;
    if extracted.failures.total > 0 {
        write!(output, " failed={} complete=0", extracted.failures.total).map_err(output_error)?;
    }
    if extracted.failures.omitted() > 0 {
        write!(output, " errors_omitted={}", extracted.failures.omitted()).map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    for failure in &extracted.failures.shown {
        writeln!(
            output,
            "error file={} code={} message={}",
            quote_metadata(&failure.subject),
            failure.code,
            quote_metadata(&failure.message)
        )
        .map_err(output_error)?;
    }
    for edge in edges {
        writeln!(
            output,
            "dependent={} line={} target={} resolution={} import={}",
            display_path(&edge.source, &root),
            edge.line,
            sanitize_metadata(&edge.target_label),
            edge.resolution,
            quote_metadata(&edge.text)
        )
        .map_err(output_error)?;
    }
    finish_partial_result(
        extracted.scanned,
        extracted.scanned.saturating_sub(extracted.failures.total),
        &extracted.failures,
        "all dependents files failed; inspect the reported file errors",
        output,
    )
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

struct DependencyExtraction<T> {
    scanned: usize,
    failures: FailureCollector,
    edges: Vec<T>,
}

fn extract_dependencies<T, F>(
    files: &[(PathBuf, Language)],
    root: &Path,
    skip: Option<&Path>,
    map_edge: F,
) -> DependencyExtraction<T>
where
    T: Send,
    F: Fn(ImportEdge) -> Option<T> + Sync,
{
    files
        .par_iter()
        .filter(|(path, _)| skip != Some(path.as_path()))
        .fold(
            || DependencyExtraction {
                scanned: 0,
                failures: FailureCollector::default(),
                edges: Vec::new(),
            },
            |mut output, (path, language)| {
                output.scanned += 1;
                match parse_syntax(path, *language) {
                    Ok(parsed) => output.edges.extend(
                        deps::imports(&parsed, root)
                            .into_iter()
                            .filter_map(&map_edge),
                    ),
                    Err(message) => {
                        output.failures.record(display_path(path, root), 2, message);
                    }
                }
                output
            },
        )
        .reduce(
            || DependencyExtraction {
                scanned: 0,
                failures: FailureCollector::default(),
                edges: Vec::new(),
            },
            |mut left, mut right| {
                left.scanned += right.scanned;
                left.failures = left.failures.merge_sorted(right.failures);
                left.edges.append(&mut right.edges);
                left
            },
        )
}

struct DependencyGraph<'a> {
    forward: BTreeMap<&'a Path, Vec<usize>>,
    reverse: BTreeMap<&'a Path, Vec<usize>>,
}

impl<'a> DependencyGraph<'a> {
    fn new(edges: &'a [LocalDependencyEdge]) -> Self {
        let mut forward = BTreeMap::<&Path, Vec<usize>>::new();
        let mut reverse = BTreeMap::<&Path, Vec<usize>>::new();
        for (index, edge) in edges.iter().enumerate() {
            forward.entry(&edge.source).or_default().push(index);
            reverse.entry(&edge.target).or_default().push(index);
        }
        Self { forward, reverse }
    }

    fn adjacent(&self, node: &Path, forward: bool) -> &[usize] {
        let selected = if forward {
            &self.forward
        } else {
            &self.reverse
        };
        selected.get(node).map(Vec::as_slice).unwrap_or_default()
    }
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
    let discovery = discover_files(
        &options.root,
        DiscoverySelection::Dependencies(target_language),
    );
    let extracted = extract_dependencies(&discovery.files, &options.root, None, |edge| {
        edge.target.map(|target| LocalDependencyEdge {
            source: edge.source,
            target,
            line: edge.line,
        })
    });
    let DependencyExtraction {
        scanned,
        failures,
        edges,
    } = extracted;
    let graph = DependencyGraph::new(&edges);
    let mut imports = Vec::with_capacity(options.max_items);
    let mut dependents = Vec::with_capacity(options.max_items);
    let mut traversed_count = 0;
    if matches!(
        options.direction,
        DependencyDirection::Imports | DependencyDirection::Both
    ) {
        traversed_count += traverse_dependencies(
            &graph,
            &edges,
            &target,
            true,
            options.depth,
            options.max_items,
            &mut imports,
        );
    }
    if matches!(
        options.direction,
        DependencyDirection::Dependents | DependencyDirection::Both
    ) {
        traversed_count += traverse_dependencies(
            &graph,
            &edges,
            &target,
            false,
            options.depth,
            options.max_items,
            &mut dependents,
        );
    }
    let traversed = match options.direction {
        DependencyDirection::Imports => imports,
        DependencyDirection::Dependents => dependents,
        DependencyDirection::Both => alternate_dependencies(imports, dependents, options.max_items),
    };
    let shown = traversed.len();
    write!(
        output,
        "# pira_codenav deps target={} root={} direction={} depth={} files={} edges={}",
        display_path(&target, &options.root),
        display_path(&options.root, cwd),
        options.direction.as_str(),
        options.depth,
        discovery.files.len(),
        traversed_count
    )
    .map_err(output_error)?;
    if shown != traversed_count {
        write!(
            output,
            " shown={} omitted={}",
            shown,
            traversed_count - shown
        )
        .map_err(output_error)?;
    }
    if failures.total > 0 {
        write!(output, " failed={} complete=0", failures.total).map_err(output_error)?;
    }
    if failures.omitted() > 0 {
        write!(output, " errors_omitted={}", failures.omitted()).map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    for failure in &failures.shown {
        writeln!(
            output,
            "error file={} code={} message={}",
            quote_metadata(&failure.subject),
            failure.code,
            quote_metadata(&failure.message)
        )
        .map_err(output_error)?;
    }
    for edge in &traversed {
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
    finish_partial_result(
        scanned,
        scanned.saturating_sub(failures.total),
        &failures,
        "all deps files failed; inspect the reported file errors",
        output,
    )
}

fn alternate_dependencies(
    imports: Vec<DependencyTraversal>,
    dependents: Vec<DependencyTraversal>,
    max_items: usize,
) -> Vec<DependencyTraversal> {
    let mut imports = imports.into_iter();
    let mut dependents = dependents.into_iter();
    let mut selected = Vec::with_capacity(max_items);
    loop {
        let before = selected.len();
        if let Some(edge) = imports.next() {
            selected.push(edge);
            if selected.len() == max_items {
                break;
            }
        }
        if let Some(edge) = dependents.next() {
            selected.push(edge);
            if selected.len() == max_items {
                break;
            }
        }
        if selected.len() == before {
            break;
        }
    }
    selected
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

fn traverse_dependencies(
    graph: &DependencyGraph<'_>,
    edges: &[LocalDependencyEdge],
    start: &Path,
    forward: bool,
    max_depth: usize,
    max_items: usize,
    output: &mut Vec<DependencyTraversal>,
) -> usize {
    let direction = if forward { "import" } else { "dependent" };
    let mut frontier = BTreeSet::from([start.to_path_buf()]);
    let mut visited_nodes = frontier.clone();
    let mut visited_edges = HashSet::new();
    let mut total = 0;
    for depth in 1..=max_depth {
        let mut candidates = frontier
            .iter()
            .flat_map(|node| graph.adjacent(node, forward).iter().copied())
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            let left = &edges[*left];
            let right = &edges[*right];
            left.source
                .cmp(&right.source)
                .then_with(|| left.target.cmp(&right.target))
                .then_with(|| left.line.cmp(&right.line))
        });
        candidates.dedup_by(|left, right| {
            let left = &edges[*left];
            let right = &edges[*right];
            left.source == right.source && left.target == right.target && left.line == right.line
        });
        let mut next = BTreeSet::new();
        for index in candidates {
            let edge = &edges[index];
            if visited_edges.insert(index) {
                total += 1;
                if output.len() < max_items {
                    output.push(DependencyTraversal {
                        depth,
                        direction,
                        source: edge.source.clone(),
                        target: edge.target.clone(),
                        line: edge.line,
                    });
                }
            }
            let adjacent = if forward { &edge.target } else { &edge.source };
            if visited_nodes.insert(adjacent.clone()) {
                next.insert(adjacent.clone());
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    total
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
        write!(
            output,
            "# pira_codenav outline file={} symbols={} shown={}",
            shown_path,
            parsed.symbols.len(),
            shown
        )
        .map_err(output_error)?;
    } else {
        write!(
            output,
            "# pira_codenav outline file={} symbols={} matched={} shown={}",
            shown_path,
            parsed.symbols.len(),
            selected.len(),
            shown
        )
        .map_err(output_error)?;
    }
    if parsed.backend == ParseBackend::Lsp {
        write!(output, " backend=lsp").map_err(output_error)?;
    }
    if !path_suffix_identifies_language(&parsed.path, parsed.language) {
        write!(output, " language={}", parsed.language.name()).map_err(output_error)?;
    }
    let omitted = selected.len().saturating_sub(shown);
    if omitted > 0 {
        write!(output, " omitted={omitted}").map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    for symbol in selected.into_iter().take(shown) {
        let indent = "  ".repeat(symbol.depth);
        write!(
            output,
            "{indent}{} {} L{}:{}-{}:{}",
            symbol.kind,
            sanitize_metadata(&symbol.qualified_name),
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
    write!(
        output,
        "# pira_codenav show file={} item={} kind={} range=L{}:{}-{}:{} hash={}",
        display_path(&parsed.path, cwd),
        symbol.qualified_name,
        symbol.kind,
        symbol.start_row + 1,
        symbol.start_column + 1,
        symbol.end_row + 1,
        symbol.end_column + 1,
        hash16(source.as_bytes())
    )
    .map_err(output_error)?;
    if parsed.backend == ParseBackend::Lsp {
        write!(output, " backend=lsp").map_err(output_error)?;
    }
    writeln!(output).map_err(output_error)?;
    let (rendered, escaped_controls) = escape_untrusted_text(source);
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
    start: usize,
    requested_end: usize,
    cwd: &Path,
    output: &mut dyn Write,
) -> CommandResult {
    if start == 0 || requested_end < start {
        return Err((2, "line range must satisfy 1 <= START <= END".into()));
    }
    let source = read_source(path).map_err(input_error)?;
    let mut line_count = usize::from(!source.is_empty());
    let mut start_byte = (start == 1 && !source.is_empty()).then_some(0);
    let mut requested_end_byte = None;
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' && index + 1 < source.len() {
            if line_count == requested_end {
                requested_end_byte = Some(index + 1);
                break;
            }
            line_count += 1;
            if line_count == start {
                start_byte = Some(index + 1);
            }
        }
    }
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
    let start_byte = start_byte.expect("validated source line has a byte offset");
    let (end, end_byte) = if let Some(end_byte) = requested_end_byte {
        (requested_end, end_byte)
    } else {
        (requested_end.min(line_count), source.len())
    };
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
        "# pira_codenav show file={} lines={}-{} range=L{}:1-{}:{} hash={}",
        display_path(path, cwd),
        start,
        end,
        start,
        end,
        final_line.len() + 1,
        hash16(selected.as_bytes())
    )
    .map_err(output_error)?;
    let (rendered, escaped_controls) = escape_untrusted_text(selected);
    render_source_boundary(output, escaped_controls)?;
    write!(output, "{rendered}").map_err(output_error)?;
    if !rendered.ends_with('\n') {
        writeln!(output).map_err(output_error)?;
    }
    writeln!(output, "--- end source ---").map_err(output_error)?;
    Ok(())
}

fn path_suffix_identifies_language(path: &Path, language: Language) -> bool {
    path.extension().is_some() && Language::infer(path).ok() == Some(language)
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

struct FileDiscovery {
    files: Vec<(PathBuf, Language)>,
    discovered: usize,
    unsupported: usize,
    ambiguous: usize,
}

#[derive(Clone, Copy)]
enum DiscoverySelection {
    Any,
    Exact(Language),
    Dependencies(Language),
}

enum DiscoveredLanguage {
    Eligible(Language),
    Unsupported,
    Ambiguous,
}

fn dependency_languages_are_compatible(target: Language, candidate: Language) -> bool {
    target == candidate
        || matches!(
            (target, candidate),
            (
                Language::C | Language::Cpp | Language::Cuda,
                Language::C | Language::Cpp | Language::Cuda
            ) | (
                Language::JavaScript | Language::TypeScript,
                Language::JavaScript | Language::TypeScript
            )
        )
}

fn classify_discovered_path(path: &Path, selection: DiscoverySelection) -> DiscoveredLanguage {
    match selection {
        DiscoverySelection::Any => {
            if Language::is_ambiguous_path(path) {
                DiscoveredLanguage::Ambiguous
            } else {
                Language::infer(path)
                    .map(DiscoveredLanguage::Eligible)
                    .unwrap_or(DiscoveredLanguage::Unsupported)
            }
        }
        DiscoverySelection::Exact(language) => {
            if language.matches_path(path) {
                DiscoveredLanguage::Eligible(language)
            } else {
                DiscoveredLanguage::Unsupported
            }
        }
        DiscoverySelection::Dependencies(target) => {
            if Language::is_ambiguous_path(path) {
                if matches!(target, Language::C | Language::Cpp | Language::Cuda) {
                    DiscoveredLanguage::Eligible(target)
                } else {
                    DiscoveredLanguage::Unsupported
                }
            } else {
                match Language::infer(path) {
                    Ok(candidate) if dependency_languages_are_compatible(target, candidate) => {
                        DiscoveredLanguage::Eligible(candidate)
                    }
                    _ => DiscoveredLanguage::Unsupported,
                }
            }
        }
    }
}

fn discover_files(root: &Path, selection: DiscoverySelection) -> FileDiscovery {
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
        match classify_discovered_path(&path, selection) {
            DiscoveredLanguage::Eligible(language) => files.push((path, language)),
            DiscoveredLanguage::Unsupported => unsupported += 1,
            DiscoveredLanguage::Ambiguous => ambiguous += 1,
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
        "pira_codenav {VERSION} — read-only code navigation\n\nUSAGE\n  pira_codenav [LANGUAGE] SUBCOMMAND [ARGS...]\n  pira_codenav help SUBCOMMAND\n\nCHOOSE A COMMAND\n  map DIRECTORY         relevant files unknown; bounded repository shape\n  find DIRECTORY QUERY  declaration name known; search parsed declarations\n  outline FILE...       file known; declarations and ranges without bodies\n  show TARGET...        exact source for the smallest selected item or line span\n  imports FILE...       direct import/include statements from known files\n  dependents FILE       files that directly import/include one known file\n  deps FILE             bounded transitive local file dependencies\n  languages             supported language names\n\nLSP SEMANTICS\n  definition LOCATION...       definition behind a use\n  implementation LOCATION...   concrete implementations\n  type-definition LOCATION...  resolved type declaration\n  references LOCATION...       semantic references\n  callers LOCATION...          incoming call hierarchy\n  callees LOCATION...          outgoing call hierarchy\n  hover LOCATION...            bounded type or documentation text\n\nTYPICAL FLOW\n  map DIRECTORY --max-items 200 -> find or outline -> show only the needed source.\n  Use an LSP semantic command when an exact relationship matters; do not substitute text matching.\n\nLANGUAGE AND LSP\n  LANGUAGE is normally inferred from a suffix or shebang. Supply it before SUBCOMMAND for an\n  extensionless/ambiguous file or to filter a directory scan. Structural commands use bundled\n  Tree-sitter and request an optional LSP only for syntax-dirty files. Semantic commands require\n  FILE:LINE:COLUMN and --lsp [LANGUAGE=]ABSOLUTE_PATH. Servers are lazy and invocation-local.\n\nOUTPUT CONTRACT\n  Output is bounded and deterministic. Predictable success fields are omitted. backend=lsp,\n  complete=0, failed/error, omitted, and truncated fields appear only when relevant. Successful\n  rows remain available when peer files or targets fail; an all-failed command returns an error.\n\nSAFETY\n  Repository source is read but never executed or edited. Exact source and hover text are framed as\n  untrusted data. A caller-supplied LSP is an external executable and may maintain its own caches.\n\nRun `pira_codenav SUBCOMMAND --help` for exact targets, options, defaults, and examples."
    )
    .map_err(output_error)
}

fn print_language_help(language: Language, output: &mut dyn Write) -> CommandResult {
    writeln!(
        output,
        "pira_codenav {} — explicit language selection\n\nUSAGE\n  pira_codenav {} SUBCOMMAND [ARGS...]\n\nBEHAVIOR\n  Use this prefix for an extensionless or ambiguous file, to restrict map/find to {}, or to select\n  {}-qualified LSP options. A conflicting recognized suffix is an error. All commands and options\n  are otherwise unchanged.\n\nRun `pira_codenav SUBCOMMAND --help` for command syntax.",
        language.name(),
        language.name(),
        language.name(),
        language.name()
    )
    .map_err(output_error)
}

const OUTLINE_HELP: &str = r#"pira_codenav outline — inspect declarations without implementation bodies

WHEN TO USE
  The file is known and you need its structure before reading source.

USAGE
  pira_codenav [LANGUAGE] outline FILE... [--match TEXT]... [--max-items N]
    [--signatures] [--selectors] [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

OPTIONS
  --match TEXT      Case-insensitive OR filter over kind, qualified name, and signature.
                    Repeat it for alternatives. It is not a regex.
  --signatures      Add signature/type detail otherwise omitted.
  --selectors       Add freshness-checked identities for later `show`.
  --max-items N     Per-file declaration limit; default 1,000.

OUTPUT AND BACKEND
  Prints declaration kind, qualified name, and exact range; bodies are omitted. Clean files use
  bundled Tree-sitter. A syntax-dirty file requires --lsp; backend=lsp then marks the recovered
  result. Successful files remain visible if another FILE fails.

NEXT
  Use `pira_codenav show FILE:LINE` for the smallest enclosing named item. Use --selectors only when
  the handoff must remain freshness-checked across edits or later turns.

EXAMPLE
  pira_codenav outline src/parser.rs --match parse --signatures"#;

const SHOW_HELP: &str = r#"pira_codenav show — retrieve one exact structural item or line span

WHEN TO USE
  You know the relevant item or range and need the smallest sufficient exact source.

USAGE
  pira_codenav [LANGUAGE] show TARGET... [--max-items N] [--max-bytes N]
    [--lsp [LANGUAGE=]ABSOLUTE_PATH] [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

TARGETS
  FILE:LINE[:COLUMN]       Selects the smallest enclosing named item; coordinates are one-based.
  FILE::QUALIFIED-NAME     Exact declaration name from outline/find.
  pira://...               Freshness-checked selector from --selectors.
  FILE:START-END           Exact inclusive line span; only as the single target.

BOUNDS AND OUTPUT
  A single structural target returns the whole item by default. Multiple targets default to 20
  deduplicated whole items and 64 KiB; --max-items and --max-bytes omit whole items rather than
  truncating source. FILE:START-END is parser-free and clamps END at EOF. Structural targets require
  --lsp only when the file is syntax-dirty. Selectors reject stale source. Returned source is framed
  as untrusted repository data.

EXAMPLES
  pira_codenav show src/parser.rs:120
  pira_codenav show src/parser.rs::Parser::parse
  pira_codenav show src/parser.rs:120-145"#;

const MAP_HELP: &str = r#"pira_codenav map — produce a bounded repository or subsystem shape

WHEN TO USE
  Start here when the relevant files are unknown.

USAGE
  pira_codenav [LANGUAGE] map DIRECTORY [--max-items N] [--lsp [LANGUAGE=]ABSOLUTE_PATH]...
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

OUTPUT AND LIMITS
  Prints compact file rows with language and representative top-level declarations. Selection is
  deterministic and balanced across directories. Default: 1,000 files; use --max-items 200 or a
  narrower DIRECTORY for an initial repository pass.

DISCOVERY AND BACKEND
  Git ignore rules are honored and symlinked directories are not followed. Without LANGUAGE, each
  supported file is inferred independently; LANGUAGE restricts the scan. Clean files use bundled
  Tree-sitter. Syntax-dirty files require a matching --lsp; backend=lsp marks only those rows.
  complete=0 and bounded errors identify gaps without discarding clean rows.

EXAMPLE
  pira_codenav map src --max-items 200"#;

const FIND_HELP: &str = r#"pira_codenav find — search declarations across a repository or subsystem

WHEN TO USE
  Use when a declaration name is known but its file is not. This searches parsed declarations, not text.

USAGE
  pira_codenav [LANGUAGE] find DIRECTORY QUERY [--exact | --regex] [--kind KIND]
    [--max-items N] [--selectors] [--signatures] [--lsp [LANGUAGE=]ABSOLUTE_PATH]...
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

MATCHING
  Default            Case-insensitive substring over qualified name, signature, and kind.
  --exact            Case-insensitive full name or qualified-name suffix.
  --regex            Rust regex syntax; case-sensitive unless the pattern requests otherwise.
  --kind KIND        Restrict declaration kind.
  --signatures       Include signature/type detail.
  --selectors        Include freshness-checked `show` targets.
  --max-items N      Result limit; default 200, maximum 100,000.

OUTPUT AND BACKEND
  Results follow stable file/declaration order. LANGUAGE restricts discovery. Clean files use bundled
  Tree-sitter; syntax-dirty files require --lsp. Clean matches remain visible when other files fail.

EXAMPLES
  pira_codenav find . Parser --exact
  pira_codenav find src '^Parser::parse$' --regex --selectors"#;

const IMPORTS_HELP: &str = r#"pira_codenav imports — inspect direct import/include statements

WHEN TO USE
  A file is known and you need its immediate import/include edges.

USAGE
  pira_codenav [LANGUAGE] imports FILE...

OUTPUT AND SCOPE
  Prints source line, exact import text, resolution status, and a conservative local target when one
  can be resolved from the current workspace. Run from the intended workspace root. External,
  dynamic, ambiguous, package-dependent, and build-dependent targets remain visibly unresolved.
  Multiple FILE successes survive peer failures.

BOUNDARY
  Requires clean native Tree-sitter syntax because LSP has no portable import graph.
  Never invokes a package manager or build system, and never executes a source file.

EXAMPLE
  pira_codenav imports src/app.py src/model.py"#;

const DEPENDENTS_HELP: &str = r#"pira_codenav dependents — inspect direct reverse file dependencies

WHEN TO USE
  A file is known and you need files whose imports/includes resolve directly to it.

USAGE
  pira_codenav [LANGUAGE] dependents FILE [--root DIRECTORY]

INPUT AND SCOPE
  FILE is relative to --root; --root defaults to the current directory. Narrow --root for large
  repositories. The scan uses the target language plus only the C/C++/CUDA or JavaScript/TypeScript
  compatibility group when applicable.

OUTPUT
  Prints each direct dependent with import line and resolution. complete=0 and bounded errors identify
  parse gaps. This is file dependency navigation, not symbol reference search. Clean native syntax is
  required because standard LSP has no portable import graph.

EXAMPLE
  pira_codenav dependents package/model.py --root src"#;

const DEPS_HELP: &str = r#"pira_codenav deps — traverse bounded local structural file dependencies

WHEN TO USE
  Direct imports/dependents are insufficient and you need a bounded transitive file graph.

USAGE
  pira_codenav [LANGUAGE] deps FILE [--direction imports|dependents|both] [--depth N]
    [--root DIRECTORY] [--max-items N]

INPUT AND OPTIONS
  FILE is relative to --root; it is not FILE:LINE or a symbol target.
  --direction VALUE   imports, dependents, or both; default both.
  --depth N           Traversal depth; default 2.
  --root DIRECTORY    Dependency workspace; default current directory.
  --max-items N       Shared edge limit; default 1,000. `both` alternates directions.

OUTPUT AND BOUNDARY
  Prints conservative local file edges with depth and direction. complete=0 marks parse gaps. It does
  not infer symbol references, calls, build-system edges, package resolution, or dynamic imports.
  Clean native syntax is required because standard LSP has no portable import graph.

EXAMPLE
  pira_codenav deps package/api.py --root src --direction both --depth 2"#;

const DEFINITION_HELP: &str = r#"pira_codenav definition — locate semantic definitions through LSP

WHEN TO USE
  You need the exact definition behind a source use.

USAGE
  pira_codenav [LANGUAGE] definition FILE:LINE:COLUMN... [--max-items N]
    --lsp [LANGUAGE=]ABSOLUTE_PATH [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

INPUT AND OUTPUT
  LOCATION is FILE:LINE:COLUMN with one-based lines and UTF-8 byte columns. The default is 20
  locations per target. Local readable locations use PIRA coordinates; other locations retain LSP
  coordinates and encoding. No source body is printed.

LSP
  --lsp must name an absolute executable. --lsp-root selects its workspace; repeat --lsp-arg for
  server arguments. Up to 32 targets reuse one server and open document per file. This command never
  guesses.

EXAMPLE
  pira_codenav definition src/app.cpp:42:17 --lsp /usr/bin/clangd --lsp-root ."#;

const IMPLEMENTATION_HELP: &str = r#"pira_codenav implementation — locate semantic implementations through LSP

WHEN TO USE
  Move from an interface, abstract declaration, or protocol use to concrete implementations.

USAGE
  pira_codenav [LANGUAGE] implementation FILE:LINE:COLUMN... [--max-items N]
    --lsp [LANGUAGE=]ABSOLUTE_PATH [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

INPUT AND OUTPUT
  LOCATION uses one-based lines and UTF-8 byte columns. Default: 20 normalized locations per target,
  no source bodies. Up to 32 targets reuse one server and document state. The server must advertise
  implementation support; this command never guesses.

EXAMPLE
  pira_codenav implementation src/api.py:18:12 --lsp /absolute/path/to/server --lsp-root ."#;

const TYPE_DEFINITION_HELP: &str = r#"pira_codenav type-definition — locate semantic type definitions through LSP

WHEN TO USE
  Navigate from an expression or value to the declaration of its resolved type.

USAGE
  pira_codenav [LANGUAGE] type-definition FILE:LINE:COLUMN... [--max-items N]
    --lsp [LANGUAGE=]ABSOLUTE_PATH [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

INPUT AND OUTPUT
  LOCATION uses one-based lines and UTF-8 byte columns. Default: 20 normalized locations per target,
  no source bodies. Up to 32 targets reuse one server and document state. The server must advertise
  type-definition support; this command never guesses.

EXAMPLE
  pira_codenav type-definition src/app.ts:30:9 --lsp /absolute/path/to/server --lsp-root ."#;

const REFERENCES_HELP: &str = r#"pira_codenav references — locate bounded semantic references through LSP

WHEN TO USE
  You need semantic uses of the item at an exact source position.

USAGE
  pira_codenav [LANGUAGE] references FILE:LINE:COLUMN... [--include-declaration]
    [--max-items N] --lsp [LANGUAGE=]ABSOLUTE_PATH
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR] [--lsp-init [LANGUAGE=]JSON_FILE]
    [--lsp-settings [LANGUAGE=]JSON_FILE]

OPTIONS AND OUTPUT
  LOCATION uses one-based lines and UTF-8 byte columns. Declarations are excluded unless
  --include-declaration is passed. Default: 200 locations per target. Headers add shown/omitted only
  when bounded. Up to 32 targets reuse one server and document state. No source bodies are printed;
  this command never performs text search.

EXAMPLE
  pira_codenav references src/lib.rs:80:14 --max-items 50 --lsp /absolute/server --lsp-root ."#;

const CALLERS_HELP: &str = r#"pira_codenav callers — inspect incoming semantic calls through LSP

WHEN TO USE
  Find functions or methods that call the item at an exact source position.

USAGE
  pira_codenav [LANGUAGE] callers FILE:LINE:COLUMN... [--max-items N]
    --lsp [LANGUAGE=]ABSOLUTE_PATH [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

OUTPUT
  Default: 100 caller relations per target and 8 compact call sites per relation. Readable locations
  are normalized. Up to 32 targets reuse one server and document state. The server must support LSP
  call hierarchy; no textual or heuristic call graph is produced.

EXAMPLE
  pira_codenav callers src/app.cpp:42:17 --lsp /usr/bin/clangd --lsp-root ."#;

const CALLEES_HELP: &str = r#"pira_codenav callees — inspect outgoing semantic calls through LSP

WHEN TO USE
  Find functions or methods called by the item at an exact source position.

USAGE
  pira_codenav [LANGUAGE] callees FILE:LINE:COLUMN... [--max-items N]
    --lsp [LANGUAGE=]ABSOLUTE_PATH [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

OUTPUT
  Default: 100 callee relations per target and 8 compact call sites per relation. Readable locations
  are normalized. Up to 32 targets reuse one server and document state. The server must support LSP
  call hierarchy; no textual or heuristic call graph is produced.

EXAMPLE
  pira_codenav callees src/app.cpp:42:17 --lsp /usr/bin/clangd --lsp-root ."#;

const HOVER_HELP: &str = r#"pira_codenav hover — retrieve bounded semantic type or documentation text through LSP

WHEN TO USE
  You need concise type, signature, or documentation context at an exact source position.

USAGE
  pira_codenav [LANGUAGE] hover FILE:LINE:COLUMN... [--max-bytes N]
    --lsp [LANGUAGE=]ABSOLUTE_PATH [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

INPUT AND OUTPUT
  LOCATION uses one-based lines and UTF-8 byte columns. --max-bytes defaults to 16 KiB per target.
  Truncation occurs only at a UTF-8 boundary and is reported in the header. Content is framed as
  untrusted LSP data. Up to 32 targets reuse one server and document state.

EXAMPLE
  pira_codenav hover src/app.py:24:9 --max-bytes 4096 --lsp /absolute/server --lsp-root ."#;

const LANGUAGES_HELP: &str = r#"pira_codenav languages — list installed language capabilities

WHEN TO USE
  Language inference or compiled support is uncertain.

USAGE
  pira_codenav languages

OUTPUT
  Prints the supported-language count followed by one canonical LANGUAGE name per line. These names
  may prefix commands and qualify LSP options.

EXAMPLE
  pira_codenav languages"#;

fn print_command_help(command: &str, output: &mut dyn Write) -> CommandResult {
    let text = match command {
        "outline" => OUTLINE_HELP,
        "show" => SHOW_HELP,
        "map" => MAP_HELP,
        "find" => FIND_HELP,
        "imports" => IMPORTS_HELP,
        "dependents" => DEPENDENTS_HELP,
        "deps" => DEPS_HELP,
        "definition" => DEFINITION_HELP,
        "implementation" => IMPLEMENTATION_HELP,
        "type-definition" => TYPE_DEFINITION_HELP,
        "references" => REFERENCES_HELP,
        "callers" => CALLERS_HELP,
        "callees" => CALLEES_HELP,
        "hover" => HOVER_HELP,
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
        "# pira_codenav languages count={}",
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
    use std::path::PathBuf;

    use super::{
        DependencyTraversal, alternate_dependencies, dependency_languages_are_compatible,
        parse_location, parse_selector,
    };
    use crate::language::Language;
    use crate::util::escape_untrusted_text;

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
        let (rendered, count) = escape_untrusted_text("a\tb\n\u{1b}c\0");
        assert_eq!(rendered, "a\tb\n\\u{1b}c\\u{0}");
        assert_eq!(count, 2);
    }

    #[test]
    fn dependency_compatibility_is_narrow_and_symmetric() {
        assert!(dependency_languages_are_compatible(
            Language::C,
            Language::Cuda
        ));
        assert!(dependency_languages_are_compatible(
            Language::TypeScript,
            Language::JavaScript
        ));
        assert!(!dependency_languages_are_compatible(
            Language::Python,
            Language::TypeScript
        ));
    }

    #[test]
    fn bidirectional_dependencies_alternate_within_the_shared_bound() {
        let edge = |direction| DependencyTraversal {
            depth: 1,
            direction,
            source: PathBuf::from(direction),
            target: PathBuf::from("target"),
            line: 1,
        };
        let selected = alternate_dependencies(
            vec![edge("import"), edge("import")],
            vec![edge("dependent"), edge("dependent")],
            3,
        );
        assert_eq!(
            selected
                .iter()
                .map(|edge| edge.direction)
                .collect::<Vec<_>>(),
            ["import", "dependent", "import"]
        );
    }
}
