//! Literal English lexical review markers, not semantic judgments or rewrites.
use regex::Regex;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{self, BufWriter, Read, Write},
    rc::Rc,
};

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

// CLI reports share each matched line and borrow lexicon strings, so input files
// can be dropped after scanning. Public owned structs remain API-compatible.
#[derive(Serialize)]
struct OccurrenceView<'a> {
    column: usize,
    span: String,
    marker: &'a str,
    category: &'a str,
}
#[derive(Serialize)]
struct WarningView<'a> {
    #[serde(serialize_with = "serialize_shared_str")]
    path: Rc<str>,
    line: usize,
    column: usize,
    span: String,
    #[serde(serialize_with = "serialize_shared_str")]
    context: Rc<str>,
    matches: Vec<OccurrenceView<'a>>,
    prompt: &'static str,
}
#[derive(Serialize)]
struct ReportView<'a> {
    advisory: &'static str,
    warnings: Vec<WarningView<'a>>,
    errors: Vec<InputError>,
}
fn serialize_shared_str<S: serde::Serializer>(
    value: &Rc<str>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value)
}

impl From<WarningView<'_>> for Warning {
    fn from(view: WarningView<'_>) -> Self {
        Self {
            path: view.path.as_ref().into(),
            line: view.line,
            column: view.column,
            span: view.span,
            context: view.context.as_ref().into(),
            matches: view
                .matches
                .into_iter()
                .map(|hit| Occurrence {
                    column: hit.column,
                    span: hit.span,
                    marker: hit.marker.into(),
                    category: hit.category.into(),
                })
                .collect(),
            prompt: view.prompt,
        }
    }
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
        match value {
            Some(c) if c.is_ascii() => c.is_ascii_alphanumeric() || c == '_',
            Some(c) => self.word.is_match(c.encode_utf8(&mut [0; 4])),
            None => false,
        }
    }

    /// Scan raw source lines. Lines/columns are one-based Unicode scalar values.
    pub fn check_text(&self, text: &str, path: &str) -> Vec<Warning> {
        self.check_shared(text, path)
            .into_iter()
            .map(Warning::from)
            .collect()
    }

    fn check_shared<'a>(&'a self, text: &str, path: &str) -> Vec<WarningView<'a>> {
        let path: Rc<str> = path.into();
        let mut warnings = Vec::new();
        // PIRA: raw lines, not Markdown or semantic cue scope. No context exemptions.
        for (index, line) in source_lines(text).into_iter().enumerate() {
            let mut context = None;
            let mut hits = Vec::new();
            for cue in &self.cues {
                let mut offset = 0;
                while let Some(found) = cue.expression.find_at(line, offset) {
                    let start = found.start();
                    let end = found.end();
                    if !self.is_word(line[..start].chars().next_back())
                        && !self.is_word(line[end..].chars().next())
                    {
                        hits.push((start, end, cue));
                    }
                    // Advance from the start, not end: retain self-overlapping phrases.
                    offset = start + line[start..].chars().next().unwrap().len_utf8();
                }
            }
            hits.sort_by(|a, b| {
                (&a.0, &a.2.category, &a.2.marker).cmp(&(&b.0, &b.2.category, &b.2.marker))
            });
            let mut group: Option<WarningView<'_>> = None;
            let mut group_end = 0;
            let mut previous_start = 0;
            let mut column = 1;
            for (start, end, cue) in hits {
                // Sorted starts let us count each Unicode scalar prefix once,
                // rather than rescanning from the line start for every hit.
                column += line[previous_start..start].chars().count();
                previous_start = start;
                let hit = OccurrenceView {
                    column,
                    span: line[start..end].into(),
                    marker: &cue.marker,
                    category: &cue.category,
                };
                if group.is_none() || start >= group_end {
                    if let Some(previous) = group.take() {
                        warnings.push(previous);
                    }
                    group_end = end;
                    group = Some(WarningView {
                        path: Rc::clone(&path),
                        line: index + 1,
                        column,
                        span: hit.span.clone(),
                        context: Rc::clone(context.get_or_insert_with(|| Rc::from(line))),
                        matches: vec![hit],
                        prompt: PROMPT,
                    });
                } else {
                    // Append only the newly covered bytes, including UTF-8 and spacing.
                    if end > group_end {
                        group.as_mut().unwrap().span.push_str(&line[group_end..end]);
                        group_end = end;
                    }
                    group.as_mut().unwrap().matches.push(hit);
                }
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

fn open_input(path: &str) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Follow symlinks, but do not block on FIFOs or acquire a controlling TTY.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    options.open(path)
}

fn read_opened_file(mut file: fs::File) -> Result<String, String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, RawHandle};
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileType(handle: RawHandle) -> u32;
        }
        const FILE_TYPE_DISK: u32 = 1;
        // SAFETY: this is a type query on the live, owned File handle.
        if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
            return Err("expected an existing regular file".into());
        }
    }
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("expected an existing regular file".into());
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|e| e.to_string())?;
    Ok(text)
}

fn read_file(path: &str) -> Result<String, String> {
    read_opened_file(open_input(path).map_err(|e| e.to_string())?)
}
fn quote(value: &str) -> io::Result<String> {
    // Match prototype text escaping, including non-BMP scalars as surrogate pairs.
    let encoded = serde_json::to_string(value).map_err(io::Error::other)?;
    let mut output = String::new();
    for c in encoded.chars() {
        if c.is_ascii() && c != '\u{7f}' {
            output.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                output.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    Ok(output)
}
fn emit(report: &ReportView<'_>, json: bool, output: &mut dyn Write) -> io::Result<()> {
    if json {
        serde_json::to_writer_pretty(&mut *output, report).map_err(io::Error::other)?;
        return writeln!(output);
    }
    for warning in &report.warnings {
        writeln!(
            output,
            "{}:{}:{}: {}: {}",
            quote(&warning.path)?,
            warning.line,
            warning.column,
            quote(&warning.span)?,
            warning.prompt
        )?;
        for hit in &warning.matches {
            let category = quote(&hit.category)?;
            writeln!(
                output,
                "  column {} [{}] {} (marker: {})",
                hit.column,
                &category[1..category.len() - 1],
                quote(&hit.span)?,
                quote(&hit.marker)?
            )?;
        }
        writeln!(output, "  context: {}", quote(&warning.context)?)?;
    }
    for error in &report.errors {
        writeln!(
            output,
            "{}: input error: {}",
            quote(&error.path)?,
            quote(&error.reason)?
        )?;
    }
    let count: usize = report.warnings.iter().map(|w| w.matches.len()).sum();
    writeln!(
        output,
        "{count} marker occurrence(s), {} review group(s), {} input error(s). {ADVISORY}",
        report.warnings.len(),
        report.errors.len()
    )
}
fn usage_error(reason: &str, diagnostics: &mut dyn Write) -> io::Result<i32> {
    writeln!(
        diagnostics,
        "pira_write_check: {reason}\nUsage: pira_write_check [OPTIONS] FILE... (see --help)"
    )?;
    Ok(2)
}

/// Run the read-only CLI: warnings return 0, output failures 1, input/usage errors 2.
pub fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut output = BufWriter::new(stdout.lock());
    run_with_io(args, &mut output, &mut stderr.lock())
}

fn run_with_io(
    args: impl IntoIterator<Item = OsString>,
    output: &mut dyn Write,
    diagnostics: &mut dyn Write,
) -> i32 {
    let result = run_inner(args, output, diagnostics).and_then(|status| {
        output.flush()?;
        diagnostics.flush()?;
        Ok(status)
    });
    match result {
        Ok(status) => status,
        Err(error) => {
            // PIRA: diagnostics are best-effort, including when stderr itself fails.
            let _ = writeln!(diagnostics, "pira_write_check: output error: {error}");
            let _ = diagnostics.flush();
            1
        }
    }
}

fn run_inner(
    args: impl IntoIterator<Item = OsString>,
    output: &mut dyn Write,
    diagnostics: &mut dyn Write,
) -> io::Result<i32> {
    let mut args = args.into_iter().skip(1);
    let mut json = false;
    let mut lexicon = None;
    let mut files = Vec::new();
    let mut options = true;
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else {
            return usage_error("arguments must be valid Unicode", diagnostics);
        };
        match arg {
            "--" if options => options = false,
            "--help" | "-h" if options => {
                write!(
                    output,
                    "pira_write_check {VERSION}\n\n{}",
                    include_str!("help.md")
                )?;
                return Ok(0);
            }
            "--version" | "-V" if options => {
                writeln!(output, "pira_write_check {VERSION}")?;
                return Ok(0);
            }
            "--json" if options => json = true,
            "--lexicon" if options => {
                let Some(path) = args.next() else {
                    return usage_error("--lexicon requires a file", diagnostics);
                };
                let Some(path) = path.to_str() else {
                    return usage_error("lexicon path must be valid Unicode", diagnostics);
                };
                lexicon = Some(path.to_owned());
            }
            _ if options && arg.starts_with("--lexicon=") => lexicon = Some(arg[10..].into()),
            _ if options && arg.starts_with('-') => {
                let escaped = quote(arg)?;
                return usage_error(
                    &format!("unknown option: {}", &escaped[1..escaped.len() - 1]),
                    diagnostics,
                );
            }
            _ => files.push(arg.to_owned()),
        }
    }
    if files.is_empty() {
        return usage_error("at least one explicit FILE is required", diagnostics);
    }
    let cues = lexicon.as_ref().map_or_else(
        || Lexicon::parse(DEFAULT_LEXICON),
        |path| read_file(path).and_then(|s| Lexicon::parse(&s)),
    );
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    match cues {
        Err(reason) => errors.push(InputError {
            path: lexicon.unwrap_or_else(|| "<embedded lexicon>".into()),
            reason,
        }),
        Ok(ref cues) => {
            for path in files {
                match read_file(&path) {
                    Ok(text) => warnings.extend(cues.check_shared(&text, &path)),
                    Err(reason) => errors.push(InputError { path, reason }),
                }
            }
        }
    }
    let report = ReportView {
        advisory: ADVISORY,
        warnings,
        errors,
    };
    emit(&report, json, output)?;
    Ok(if report.errors.is_empty() { 0 } else { 2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ReaderFixture(std::path::PathBuf);
    impl ReaderFixture {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "write-reader-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for ReaderFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn opened_input_is_nonblocking_and_read_from_the_original_handle() {
        let fixture = ReaderFixture::new();
        let path = fixture.0.join("α.txt");
        let original = "not π no\n";
        fs::write(&path, original).unwrap();
        let file = open_input(path.to_str().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: F_GETFL queries this live descriptor and consumes no vararg.
            let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
            assert!(flags >= 0);
            assert_ne!(flags & libc::O_NONBLOCK, 0);
        }
        // Deterministic namespace replacement, not a racing/blocking FIFO test.
        fs::rename(&path, fixture.0.join("original.txt")).unwrap();
        fs::write(&path, b"\xff").unwrap();
        assert_eq!(read_opened_file(file).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn opened_nonregular_handle_is_rejected_before_reading() {
        use std::{
            net::Shutdown,
            os::{fd::OwnedFd, unix::net::UnixStream},
        };
        let (input, peer) = UnixStream::pair().unwrap();
        // Even a broken implementation reads EOF, never blocks on this fixture.
        peer.shutdown(Shutdown::Write).unwrap();
        let descriptor: OwnedFd = input.into();
        let error = read_opened_file(fs::File::from(descriptor)).unwrap_err();
        assert_eq!(error, "expected an existing regular file");
    }

    #[cfg(windows)]
    #[test]
    fn windows_character_device_is_rejected_before_reading() {
        let file = open_input(r"\\.\NUL").unwrap();
        assert_eq!(
            read_opened_file(file).unwrap_err(),
            "expected an existing regular file"
        );
    }

    struct FailingWriter {
        remaining: Option<usize>,
        fail_flush: bool,
    }
    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            match self.remaining {
                Some(0) => Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected write failure",
                )),
                Some(ref mut remaining) => {
                    let count = bytes.len().min(*remaining);
                    *remaining -= count;
                    Ok(count)
                }
                None => Ok(bytes.len()),
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush {
                Err(io::Error::other("injected flush failure"))
            } else {
                Ok(())
            }
        }
    }
    fn args(options: &[&str]) -> Vec<OsString> {
        std::iter::once("pira_write_check")
            .chain(options.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn stdout_write_and_flush_failures_return_one_for_all_output_modes() {
        for options in [
            vec!["--help"],
            vec!["--version"],
            vec!["--lexicon=", "unused"],
            vec!["--json", "--lexicon=", "unused"],
        ] {
            for (remaining, fail_flush) in [(Some(0), false), (Some(7), false), (None, true)] {
                let mut sink = FailingWriter {
                    remaining,
                    fail_flush,
                };
                let mut output = BufWriter::with_capacity(16, &mut sink);
                let mut diagnostics = Vec::new();
                assert_eq!(
                    run_with_io(args(&options), &mut output, &mut diagnostics),
                    1,
                    "{options:?}"
                );
                assert!(
                    String::from_utf8(diagnostics)
                        .unwrap()
                        .contains("output error:")
                );
            }
        }
    }

    #[test]
    fn warning_serialization_write_failures_are_returned() {
        let lexicon = Lexicon::parse(r#"{"x":["not"]}"#).unwrap();
        let report = ReportView {
            advisory: ADVISORY,
            warnings: lexicon.check_shared("not", "fixture"),
            errors: Vec::new(),
        };
        for json in [false, true] {
            let mut output = FailingWriter {
                remaining: Some(7),
                fail_flush: false,
            };
            assert!(emit(&report, json, &mut output).is_err());
        }
    }

    #[test]
    fn text_report_escapes_del_in_every_quoted_field() {
        let lexicon = Lexicon::parse(r#"{"c\u007f":["a\u007fb"]}"#).unwrap();
        let report = ReportView {
            advisory: ADVISORY,
            warnings: lexicon.check_shared("a\u{7f}b tail\u{7f}", "p\u{7f}"),
            errors: vec![InputError {
                path: "e\u{7f}".into(),
                reason: "r\u{7f}".into(),
            }],
        };
        let mut text = Vec::new();
        emit(&report, false, &mut text).unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            format!(
                "\"p\\u007f\":1:1: \"a\\u007fb\": {PROMPT}\n  column 1 [c\\u007f] \"a\\u007fb\" (marker: \"a\\u007fb\")\n  context: \"a\\u007fb tail\\u007f\"\n\"e\\u007f\": input error: \"r\\u007f\"\n1 marker occurrence(s), 1 review group(s), 1 input error(s). {ADVISORY}\n"
            )
        );
        let mut json = Vec::new();
        emit(&report, true, &mut json).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(json["warnings"][0]["path"], "p\u{7f}");
        assert_eq!(json["warnings"][0]["span"], "a\u{7f}b");
        assert_eq!(json["warnings"][0]["context"], "a\u{7f}b tail\u{7f}");
        assert_eq!(json["warnings"][0]["matches"][0]["category"], "c\u{7f}");
        assert_eq!(json["errors"][0]["reason"], "r\u{7f}");
    }

    #[test]
    fn stderr_write_and_flush_failures_do_not_panic() {
        for (remaining, fail_flush) in [(Some(0), false), (Some(7), false), (None, true)] {
            let mut diagnostics = FailingWriter {
                remaining,
                fail_flush,
            };
            let mut output = Vec::new();
            assert_eq!(run_with_io(args(&[]), &mut output, &mut diagnostics), 1);
            assert!(output.is_empty());
        }
        let mut output = FailingWriter {
            remaining: Some(0),
            fail_flush: false,
        };
        let mut diagnostics = FailingWriter {
            remaining: Some(0),
            fail_flush: true,
        };
        assert_eq!(
            run_with_io(args(&["--version"]), &mut output, &mut diagnostics),
            1
        );
    }
}
