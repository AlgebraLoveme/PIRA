use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseState {
    Ok,
    Recovered,
    Partial,
}

impl ParseState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Recovered => "recovered",
            Self::Partial => "partial",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub kind: &'static str,
    pub qualified_name: String,
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

#[derive(Clone, Debug)]
pub struct ImportEdge {
    pub source: PathBuf,
    pub line: usize,
    pub text: String,
    pub target: Option<PathBuf>,
    pub target_label: String,
    pub resolution: &'static str,
}
