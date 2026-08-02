use std::borrow::Cow;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

pub fn read_source(path: &Path) -> Result<String, String> {
    let file =
        File::open(path).map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("not a regular file: {}", path.display()));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "source file exceeds the {} MiB safety limit: {}",
            MAX_FILE_BYTES / (1024 * 1024),
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_FILE_BYTES) as usize);
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "source file exceeds the {} MiB safety limit: {}",
            MAX_FILE_BYTES / (1024 * 1024),
            path.display()
        ));
    }
    if bytes.contains(&0) {
        return Err(format!(
            "source contains NUL bytes and is treated as binary: {}",
            path.display()
        ));
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    String::from_utf8(bytes.to_vec()).map_err(|error| {
        format!(
            "source is not valid UTF-8 or cannot be read: {}: {error}",
            path.display()
        )
    })
}

pub fn display_path(path: &Path, cwd: &Path) -> String {
    let shown = path.strip_prefix(cwd).unwrap_or(path);
    let text = shown.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        ".".into()
    } else {
        sanitize_metadata(&text)
    }
}

pub fn normalize_lexically(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

pub fn absolute_lexical(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_lexically(path)
    } else {
        normalize_lexically(&cwd.join(path))
    }
}

/// Return one cheap, deterministic correction for a missing path.
///
/// Keep this deliberately bounded: command errors should be helpful without
/// turning a typo into a repository-wide search. The two cases cover common
/// agent mistakes: naming a Rust module as `name.rs` when it is `name/mod.rs`,
/// and accidentally prefixing a repository-root file with a subdirectory.
pub fn nearby_existing_path(path: &Path, cwd: &Path, want_file: bool) -> Option<PathBuf> {
    let matches_kind = |candidate: &Path| {
        if want_file {
            candidate.is_file()
        } else {
            candidate.is_file() || candidate.is_dir()
        }
    };
    if want_file && path.extension().is_some_and(|extension| extension == "rs") {
        let module = path.parent()?.join(path.file_stem()?).join("mod.rs");
        if module != path && module.is_file() {
            return Some(module);
        }
    }
    let root_candidate = cwd.join(path.file_name()?);
    (root_candidate != path && matches_kind(&root_candidate)).then_some(root_candidate)
}

#[derive(Clone, Copy)]
pub enum PathExpectation {
    File,
    Directory,
    FileOrDirectory,
}

pub fn missing_path_message(
    command: &str,
    kind: &str,
    path: &Path,
    cwd: &Path,
    expectation: PathExpectation,
) -> String {
    let mut message = format!(
        "{command} {kind} does not exist: {}",
        display_path(path, cwd)
    );
    let want_file = matches!(expectation, PathExpectation::File);
    if let Some(candidate) =
        nearby_existing_path(path, cwd, want_file).filter(|candidate| match expectation {
            PathExpectation::File => candidate.is_file(),
            PathExpectation::Directory => candidate.is_dir(),
            PathExpectation::FileOrDirectory => candidate.is_file() || candidate.is_dir(),
        })
    {
        message.push_str(&format!(
            "; did you mean `{}`?",
            display_path(&candidate, cwd)
        ));
    } else if path.strip_prefix(cwd).is_ok() {
        message.push_str(&format!("; current directory is `{}`", cwd.display()));
    }
    message
}

pub fn repository_path_penalty(path: &Path) -> usize {
    let mut penalty = 0;
    for component in path.components() {
        let raw = component.as_os_str().to_string_lossy();
        let name = raw.to_ascii_lowercase();
        if name.starts_with('.') {
            penalty += 8;
        }
        if matches!(
            name.as_str(),
            "vendor"
                | "vendors"
                | "third_party"
                | "third-party"
                | "node_modules"
                | "generated"
                | "gen"
                | "ossfuzz"
                | "oss-fuzz"
                | "target"
                | "dist"
                | "build"
        ) {
            penalty += 12;
        } else if matches!(
            name.as_str(),
            "test"
                | "tests"
                | "testing"
                | "fixtures"
                | "fixture"
                | "examples"
                | "example"
                | "bench"
                | "benches"
                | "benchmark"
                | "benchmarks"
        ) {
            penalty += 4;
        } else if matches!(name.as_str(), "doc" | "docs" | "documentation") {
            penalty += 1;
        }
        if name.starts_with("test_")
            || name.ends_with("-test")
            || name.ends_with("_test")
            || name.ends_with("_test.go")
            || name.ends_with("_test.rs")
            || name.ends_with(".generated.rs")
            || name.ends_with(".pb.go")
            || name.ends_with(".pb.cc")
            || name.ends_with(".pb.h")
        {
            penalty += 4;
        }
        if matches!(
            name.as_str(),
            "license" | "license.md" | "license.txt" | "copying" | "changelog.md"
        ) || name.ends_with(".lock")
        {
            penalty += 8;
        }
        if name.starts_with('_') && name != "__init__.py" {
            penalty += 1;
        }
    }
    penalty
}

pub fn is_broad_map_fixture(path: &Path, root: &Path, document: bool) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        let names = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect::<Vec<_>>();
        (document
            && names
                .iter()
                .any(|name| matches!(name.as_str(), "test" | "tests")))
            || names.into_iter().any(|name| {
                matches!(
                    name.as_str(),
                    "fixture"
                        | "fixtures"
                        | "testdata"
                        | "test-data"
                        | "test_data"
                        | "corpus"
                        | "fuzz"
                        | "fuzzing"
                        | "ossfuzz"
                        | "oss-fuzz"
                        | "seeds"
                        | "snapshots"
                        | "snapshot"
                        | "golden"
                        | "expected"
                        | "invalid"
                        | "vendor"
                        | "third_party"
                        | "third-party"
                        | "node_modules"
                )
            })
    })
}

pub fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn hash16(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(16);
    for byte in &digest[..8] {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

pub fn sanitize_metadata(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            use std::fmt::Write as _;
            let _ = write!(result, "\\u{{{:x}}}", character as u32);
        } else {
            result.push(character);
        }
    }
    result
}

pub fn escape_untrusted_text(value: &str) -> (Cow<'_, str>, usize) {
    let unsafe_control =
        |character: char| character.is_control() && !matches!(character, '\n' | '\r' | '\t');
    if !value.chars().any(unsafe_control) {
        return (Cow::Borrowed(value), 0);
    }

    let mut escaped = 0;
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if unsafe_control(character) {
            use std::fmt::Write as _;
            let _ = write!(output, "\\u{{{:x}}}", character as u32);
            escaped += 1;
        } else {
            output.push(character);
        }
    }
    (Cow::Owned(output), escaped)
}

pub fn quote_metadata(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for character in sanitize_metadata(value).chars() {
        match character {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            _ => result.push(character),
        }
    }
    result.push('"');
    result
}

pub fn percent_encode(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~') {
            result.push(*byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}

pub fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err("truncated percent escape".into());
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| "invalid percent escape")?;
            output.push(u8::from_str_radix(hex, 16).map_err(|_| "invalid percent escape")?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|_| "selector contains invalid UTF-8".into())
}

pub fn source_slice<'a>(source: &'a str, start: usize, end: usize) -> Cow<'a, str> {
    match source.get(start..end) {
        Some(value) => Cow::Borrowed(value),
        None => Cow::Owned(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{percent_decode, percent_encode, repository_path_penalty};

    #[test]
    fn percent_encoding_round_trips_unicode_and_reserved_bytes() {
        let input = "src/naïve file.rs::Thing";
        let encoded = percent_encode(input);
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains(' '));
        assert_eq!(percent_decode(&encoded).expect("valid encoding"), input);
    }

    #[test]
    fn malformed_percent_encoding_is_rejected() {
        assert!(percent_decode("bad%2").is_err());
        assert!(percent_decode("bad%ZZ").is_err());
    }

    #[test]
    fn repository_path_penalty_recognizes_delimited_test_directories() {
        assert!(
            repository_path_penalty(Path::new("internal/parser-test/case.go"))
                > repository_path_penalty(Path::new("internal/parser/case.go"))
        );
        assert!(
            repository_path_penalty(Path::new("internal/parser_test/case.go"))
                > repository_path_penalty(Path::new("internal/parser/case.go"))
        );
    }
}
