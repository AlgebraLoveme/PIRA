//! Literal English lexical review markers, not semantic judgments or rewrites.
use regex::Regex;
use serde::Serialize;
use std::{collections::BTreeMap, ffi::OsString, fs, path::Path};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_LEXICON: &str = include_str!("lexicon.json");
pub const ADVISORY: &str = "Configured English marker occurrences only; review necessity in context. No semantic verdict or clean-scan quality guarantee.";
pub const PROMPT: &str = "Review whether this marker is necessary in context.";

/// Validated, literal marker inventory. Matching never treats entries as regexes.
pub struct Lexicon {
    cues: Vec<Cue>,
    word: Regex,
}
struct Cue {
    category: String,
    marker: String,
    expression: Regex,
}

/// One exact marker occurrence within its group’s source line.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Occurrence {
    pub column: usize,
    pub span: String,
    pub marker: String,
    pub category: String,
}
/// A transitively overlapping group with every individual occurrence.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Warning {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub span: String,
    pub context: String,
    pub matches: Vec<Occurrence>,
    pub prompt: &'static str,
}
/// An unreadable input or invalid lexicon, retaining the supplied path.
#[derive(Debug, Serialize)]
pub struct InputError {
    pub path: String,
    pub reason: String,
}
/// Deterministic advisory warnings and input errors in argument order.
#[derive(Debug, Serialize)]
pub struct Report {
    pub advisory: &'static str,
    pub warnings: Vec<Warning>,
    pub errors: Vec<InputError>,
}

fn literal_expression(marker: &str) -> String {
    // Python IGNORECASE also equates dotted/dotless I with English I/i.
    marker
        .split(' ')
        .map(|part| {
            part.chars()
                .map(|c| match c {
                    'i' | 'I' | 'İ' | 'ı' => "[iIİı]".to_owned(),
                    _ => regex::escape(&c.to_string()),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("[ \\t]+")
}

impl Lexicon {
    /// Parse a nonempty category-to-marker-list JSON object; reject invalid entries.
    pub fn parse(source: &str) -> Result<Self, String> {
        let data: BTreeMap<String, Vec<String>> = serde_json::from_str(source)
            .map_err(|e| format!("expected category-to-marker-list JSON: {e}"))?;
        if data.is_empty() {
            return Err("lexicon must be a nonempty object of category: [markers]".into());
        }
        let word = Regex::new(r"^[\p{L}\p{N}_]$").unwrap();
        let mut cues = Vec::new();
        for (category, markers) in data {
            if category.chars().all(python_whitespace) || markers.is_empty() {
                return Err("each category must have a name and a nonempty marker list".into());
            }
            let mut seen: Vec<Regex> = Vec::new();
            for marker in markers {
                if marker.is_empty()
                    || marker
                        .split(python_whitespace)
                        .filter(|p| !p.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                        != marker
                    || !word.is_match(&marker.chars().next().unwrap().to_string())
                    || !word.is_match(&marker.chars().last().unwrap().to_string())
                {
                    return Err(
                        "markers must have word-character ends and single spaces between words"
                            .into(),
                    );
                }
                if seen.iter().any(|pattern| pattern.is_match(&marker)) {
                    return Err(format!("duplicate marker in {category}: {marker}"));
                }
                let literal = literal_expression(&marker);
                let expression = Regex::new(&format!("(?i){literal}"))
                    .map_err(|e| format!("cannot compile literal marker {marker:?}: {e}"))?;
                seen.push(
                    Regex::new(&format!("(?i)\\A(?:{literal})\\z"))
                        .map_err(|e| format!("cannot compile literal marker {marker:?}: {e}"))?,
                );
                cues.push(Cue {
                    category: category.clone(),
                    marker,
                    expression,
                });
            }
        }
        Ok(Self { cues, word })
    }

    fn is_word(&self, value: Option<char>) -> bool {
        value.is_some_and(|c| self.word.is_match(&c.to_string()))
    }

    /// Scan raw source lines. Lines/columns are one-based Unicode scalar values.
    pub fn check_text(&self, text: &str, path: &str) -> Vec<Warning> {
        let mut warnings: Vec<Warning> = Vec::new();
        // PIRA: raw lines, not Markdown or semantic cue scope. No context exemptions.
        for (index, line) in source_lines(text).into_iter().enumerate() {
            let mut hits = Vec::new();
            for cue in &self.cues {
                let mut offset = 0;
                while let Some(found) = cue.expression.find_at(line, offset) {
                    let start = found.start();
                    let end = found.end();
                    if !self.is_word(line[..start].chars().next_back())
                        && !self.is_word(line[end..].chars().next())
                    {
                        hits.push((
                            start,
                            end,
                            Occurrence {
                                column: line[..start].chars().count() + 1,
                                span: found.as_str().into(),
                                marker: cue.marker.clone(),
                                category: cue.category.clone(),
                            },
                        ));
                    }
                    // Advance from the start, not end: retain self-overlapping phrases.
                    offset = start + line[start..].chars().next().unwrap().len_utf8();
                }
            }
            hits.sort_by(|a, b| {
                (&a.0, &a.2.category, &a.2.marker).cmp(&(&b.0, &b.2.category, &b.2.marker))
            });
            let mut group: Option<Warning> = None;
            let mut group_start = 0;
            let mut group_end = 0;
            for (start, end, hit) in hits {
                if group.is_none() || start >= group_end {
                    if let Some(previous) = group.take() {
                        warnings.push(previous);
                    }
                    group_start = start;
                    group_end = end;
                    group = Some(Warning {
                        path: path.into(),
                        line: index + 1,
                        column: hit.column,
                        span: hit.span.clone(),
                        context: line.into(),
                        matches: Vec::new(),
                        prompt: PROMPT,
                    });
                } else {
                    group_end = group_end.max(end);
                    group.as_mut().unwrap().span = line[group_start..group_end].into();
                }
                group.as_mut().unwrap().matches.push(hit);
            }
            if let Some(previous) = group {
                warnings.push(previous);
            }
        }
        warnings
    }
}

fn python_whitespace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}
fn line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}
fn source_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if line_break(c) {
            lines.push(&text[start..index]);
            start = index + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                chars.next();
                start += 1;
            }
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn read_file(path: &str) -> Result<String, String> {
    if !Path::new(path).is_file() {
        return Err("expected an existing regular file".into());
    }
    fs::read_to_string(path).map_err(|e| e.to_string())
}
fn quote(value: &str) -> String {
    // Match prototype text escaping, including non-BMP scalars as surrogate pairs.
    let encoded = serde_json::to_string(value).unwrap();
    let mut output = String::new();
    for c in encoded.chars() {
        if c.is_ascii() {
            output.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                output.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    output
}
fn emit(report: &Report, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(report).unwrap());
        return;
    }
    for warning in &report.warnings {
        println!(
            "{}:{}:{}: {}: {}",
            quote(&warning.path),
            warning.line,
            warning.column,
            quote(&warning.span),
            warning.prompt
        );
        for hit in &warning.matches {
            let category = quote(&hit.category);
            println!(
                "  column {} [{}] {} (marker: {})",
                hit.column,
                &category[1..category.len() - 1],
                quote(&hit.span),
                quote(&hit.marker)
            );
        }
        println!("  context: {}", quote(&warning.context));
    }
    for error in &report.errors {
        println!(
            "{}: input error: {}",
            quote(&error.path),
            quote(&error.reason)
        );
    }
    let count: usize = report.warnings.iter().map(|w| w.matches.len()).sum();
    println!(
        "{count} marker occurrence(s), {} review group(s), {} input error(s). {ADVISORY}",
        report.warnings.len(),
        report.errors.len()
    );
}
fn help() {
    print!("pira_write_check {VERSION}\n\n{}", include_str!("help.md"));
}
fn usage_error(reason: &str) -> i32 {
    eprintln!("pira_write_check: {reason}\nUsage: pira_write_check [OPTIONS] FILE... (see --help)");
    2
}

/// Run the read-only CLI; warnings return zero, input or usage errors return two.
pub fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    let mut args = args.into_iter().skip(1);
    let mut json = false;
    let mut lexicon = None;
    let mut files = Vec::new();
    let mut options = true;
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else {
            return usage_error("arguments must be valid Unicode");
        };
        match arg {
            "--" if options => options = false,
            "--help" | "-h" if options => {
                help();
                return 0;
            }
            "--version" | "-V" if options => {
                println!("pira_write_check {VERSION}");
                return 0;
            }
            "--json" if options => json = true,
            "--lexicon" if options => {
                let Some(path) = args.next() else {
                    return usage_error("--lexicon requires a file");
                };
                let Some(path) = path.to_str() else {
                    return usage_error("lexicon path must be valid Unicode");
                };
                lexicon = Some(path.to_owned());
            }
            _ if options && arg.starts_with("--lexicon=") => lexicon = Some(arg[10..].into()),
            _ if options && arg.starts_with('-') => {
                return usage_error(&format!("unknown option: {arg}"));
            }
            _ => files.push(arg.to_owned()),
        }
    }
    if files.is_empty() {
        return usage_error("at least one explicit FILE is required");
    }
    let mut report = Report {
        advisory: ADVISORY,
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let cues = lexicon.as_ref().map_or_else(
        || Lexicon::parse(DEFAULT_LEXICON),
        |path| read_file(path).and_then(|s| Lexicon::parse(&s)),
    );
    match cues {
        Err(reason) => report.errors.push(InputError {
            path: lexicon.unwrap_or_else(|| "<embedded lexicon>".into()),
            reason,
        }),
        Ok(cues) => {
            for path in files {
                match read_file(&path) {
                    Ok(text) => report.warnings.extend(cues.check_text(&text, &path)),
                    Err(reason) => report.errors.push(InputError { path, reason }),
                }
            }
        }
    }
    emit(&report, json);
    if report.errors.is_empty() { 0 } else { 2 }
}
