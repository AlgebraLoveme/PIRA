use std::path::{Path, PathBuf};

use crate::model::Node;

pub const DEFAULT_EXTENSION: &str = "txt";

#[derive(Debug)]
pub enum ParseError {
    Empty,
    Io(std::io::Error),
}

pub struct Parser {
    root: PathBuf,
}

impl Parser {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn parse(&self, source: &str) -> Result<Node, ParseError> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return Err(ParseError::Empty);
        }
        Ok(Node::new(trimmed))
    }

    pub fn path_for(&self, stem: &str) -> PathBuf {
        self.root.join(format!("{stem}.{DEFAULT_EXTENSION}"))
    }
}

pub fn is_supported(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == DEFAULT_EXTENSION)
}

pub fn résumé(nodes: &[Node]) -> usize {
    nodes.iter().map(|node| node.text.len()).sum()
}
