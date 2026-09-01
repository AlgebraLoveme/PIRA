use std::path::PathBuf;

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct SymbolPath {
    segments: Vec<SymbolPathSegment>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SymbolPathSegment {
    Name(String),
    Index(usize),
}

impl SymbolPath {
    pub fn from_names(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            segments: names.into_iter().map(SymbolPathSegment::Name).collect(),
        }
    }

    pub fn child_name(&self, name: impl Into<String>) -> Self {
        let mut path = self.clone();
        path.segments.push(SymbolPathSegment::Name(name.into()));
        path
    }

    pub fn child_index(&self, index: usize) -> Self {
        let mut path = self.clone();
        path.segments.push(SymbolPathSegment::Index(index));
        path
    }

    pub fn extend_names(&self, names: impl IntoIterator<Item = String>) -> Self {
        let mut path = self.clone();
        path.segments
            .extend(names.into_iter().map(SymbolPathSegment::Name));
        path
    }

    pub fn last_name(&self) -> Option<&str> {
        self.segments
            .iter()
            .rev()
            .find_map(|segment| match segment {
                SymbolPathSegment::Name(name) => Some(name.as_str()),
                SymbolPathSegment::Index(_) => None,
            })
    }

    pub fn ends_with(&self, suffix: &Self) -> bool {
        !suffix.segments.is_empty() && self.segments.ends_with(&suffix.segments)
    }

    pub fn canonical(&self) -> String {
        let mut output = String::new();
        for segment in &self.segments {
            match segment {
                SymbolPathSegment::Name(name) => {
                    if !output.is_empty() {
                        output.push_str("::");
                    }
                    if is_bare_segment(name) {
                        output.push_str(name);
                    } else {
                        output.push('[');
                        output.push_str(
                            &serde_json::to_string(name)
                                .expect("serializing a symbol path segment cannot fail"),
                        );
                        output.push(']');
                    }
                }
                SymbolPathSegment::Index(index) => {
                    output.push('[');
                    output.push_str(&index.to_string());
                    output.push(']');
                }
            }
        }
        output
    }

    pub fn legacy_document(&self) -> String {
        let mut output = String::new();
        for segment in &self.segments {
            match segment {
                SymbolPathSegment::Name(name) if is_bare_document_segment(name) => {
                    if !output.is_empty() {
                        output.push('.');
                    }
                    output.push_str(name);
                }
                SymbolPathSegment::Name(name) => {
                    output.push('[');
                    output.push_str(
                        &serde_json::to_string(name)
                            .expect("serializing a document path segment cannot fail"),
                    );
                    output.push(']');
                }
                SymbolPathSegment::Index(index) => {
                    output.push('[');
                    output.push_str(&index.to_string());
                    output.push(']');
                }
            }
        }
        output
    }

    pub fn legacy_code(&self, separator: &str) -> String {
        self.segments
            .iter()
            .filter_map(|segment| match segment {
                SymbolPathSegment::Name(name) => Some(name.as_str()),
                SymbolPathSegment::Index(_) => None,
            })
            .collect::<Vec<_>>()
            .join(separator)
    }

    pub fn parse_canonical(value: &str) -> Option<Self> {
        if value.is_empty() {
            return Some(Self::default());
        }
        let bytes = value.as_bytes();
        let mut index = 0;
        let mut segments = Vec::new();
        let mut expect_name = true;
        while index < bytes.len() {
            if !expect_name && bytes[index] == b'[' {
                let end = value[index + 1..].find(']')? + index + 1;
                let raw = &value[index + 1..end];
                if raw.bytes().all(|byte| byte.is_ascii_digit()) && !raw.is_empty() {
                    segments.push(SymbolPathSegment::Index(raw.parse().ok()?));
                    index = end + 1;
                    if index == bytes.len() {
                        break;
                    }
                    if value[index..].starts_with("::") {
                        index += 2;
                        expect_name = true;
                        if index == bytes.len() {
                            return None;
                        }
                    } else if bytes[index] != b'[' {
                        return None;
                    }
                    continue;
                }
                return None;
            }
            let (segment, next) = if value[index..].starts_with("[\"") {
                parse_quoted_segment(value, index)?
            } else {
                let end = value[index..]
                    .find("::")
                    .map_or(value.len(), |offset| index + offset);
                let end = value[index..end]
                    .find('[')
                    .map_or(end, |offset| index + offset);
                if end == index || !is_bare_segment(&value[index..end]) {
                    return None;
                }
                (value[index..end].to_owned(), end)
            };
            segments.push(SymbolPathSegment::Name(segment));
            index = next;
            expect_name = false;
            if index == bytes.len() {
                break;
            }
            if value[index..].starts_with("::") {
                index += 2;
                expect_name = true;
                if index == bytes.len() {
                    return None;
                }
            } else if bytes[index] != b'[' {
                return None;
            }
        }
        (!expect_name).then_some(Self { segments })
    }
}

fn parse_quoted_segment(value: &str, start: usize) -> Option<(String, usize)> {
    let mut escaped = false;
    for (offset, character) in value[start + 2..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => {
                let quote_end = start + 2 + offset + character.len_utf8();
                if value.as_bytes().get(quote_end) != Some(&b']') {
                    return None;
                }
                let json = &value[start + 1..quote_end];
                return serde_json::from_str(json)
                    .ok()
                    .map(|segment| (segment, quote_end + 1));
            }
            _ => {}
        }
    }
    None
}

fn is_bare_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
}

fn is_bare_document_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-' | '$'))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseBackend {
    Native,
    Lsp,
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub kind: &'static str,
    pub path: SymbolPath,
    pub qualified_name: String,
    pub legacy_qualified_name: String,
    pub signature: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_row: usize,
    pub start_column: usize,
    pub end_row: usize,
    pub end_column: usize,
    pub depth: usize,
}

impl Symbol {
    pub fn name_matches(&self, query: &str) -> bool {
        self.qualified_name == query || self.legacy_qualified_name == query
    }

    pub fn name_suffix_matches(&self, query: &str) -> bool {
        SymbolPath::parse_canonical(query).is_some_and(|path| self.path.ends_with(&path))
            || legacy_suffix_matches(&self.legacy_qualified_name, query)
    }

    pub fn contains_line(&self, one_based_line: usize) -> bool {
        let start = self.start_row + 1;
        let end = self.end_row + 1;
        one_based_line >= start && one_based_line <= end
    }

    pub fn contains_position(&self, one_based_line: usize, one_based_column: usize) -> bool {
        let point = (
            one_based_line.saturating_sub(1),
            one_based_column.saturating_sub(1),
        );
        point >= (self.start_row, self.start_column) && point < (self.end_row, self.end_column)
    }

    pub fn byte_len(&self) -> usize {
        self.end_byte.saturating_sub(self.start_byte)
    }
}

fn legacy_suffix_matches(candidate: &str, query: &str) -> bool {
    candidate == query
        || candidate.strip_suffix(query).is_some_and(|prefix| {
            prefix.ends_with('.')
                || prefix.ends_with("::")
                || prefix.ends_with('\\')
                || prefix.ends_with(" > ")
        })
}

#[cfg(test)]
mod tests {
    use super::{SymbolPath, SymbolPathSegment};

    #[test]
    fn canonical_paths_round_trip_names_indices_and_quoted_segments() {
        let path = SymbolPath::from_names(["jobs".into(), "release".into()])
            .child_name("title :: advanced")
            .child_index(2)
            .child_name("résumé")
            .child_name("quote \" slash \\ bracket ]");
        let rendered = "jobs::release::[\"title :: advanced\"][2]::résumé::[\"quote \\\" slash \\\\ bracket ]\"]";
        assert_eq!(path.canonical(), rendered);
        assert_eq!(SymbolPath::parse_canonical(rendered), Some(path));
    }

    #[test]
    fn canonical_parser_rejects_incomplete_or_malformed_paths() {
        for value in ["::name", "name::", "name:::child", "[\"unterminated\"]x"] {
            assert!(SymbolPath::parse_canonical(value).is_none(), "{value}");
        }
    }

    #[test]
    fn document_legacy_rendering_remains_unambiguous() {
        let path = SymbolPath {
            segments: vec![
                SymbolPathSegment::Name("root".into()),
                SymbolPathSegment::Name("a.b".into()),
                SymbolPathSegment::Index(2),
            ],
        };
        assert_eq!(path.canonical(), "root::[\"a.b\"][2]");
        assert_eq!(path.legacy_document(), "root[\"a.b\"][2]");
        assert_eq!(
            SymbolPath::from_names(["$shell".into()]).canonical(),
            "[\"$shell\"]"
        );
        assert_eq!(
            SymbolPath::from_names(["$shell".into()]).legacy_document(),
            "$shell"
        );
    }
}

#[derive(Clone, Debug)]
pub struct ImportEdge {
    pub source: PathBuf,
    pub line: usize,
    pub text: String,
    pub target: Option<PathBuf>,
    pub target_label: String,
    pub resolution: &'static str,
}
