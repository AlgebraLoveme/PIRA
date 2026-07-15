use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::language::Language;
use crate::model::Symbol;

mod client;
mod protocol;

use client::LspClient;
pub use protocol::{LspHover, LspLocation, LspRange, file_path_from_uri, normalize_range};

#[derive(Clone, Debug)]
pub struct LspConfig {
    executable: PathBuf,
    arguments: Vec<String>,
    root: PathBuf,
}

#[derive(Default)]
pub struct LspConfigs {
    pub default: Option<LspConfig>,
    pub languages: BTreeMap<Language, LspConfig>,
}

impl LspConfig {
    pub fn new(executable: PathBuf, arguments: Vec<String>, root: PathBuf) -> Result<Self, String> {
        if !executable.is_absolute() {
            return Err("--lsp requires an absolute path to an executable".into());
        }
        let metadata = fs::metadata(&executable).map_err(|error| {
            format!(
                "cannot inspect LSP executable {}: {error}",
                executable.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "LSP executable is not a regular file: {}",
                executable.display()
            ));
        }
        if !root.is_dir() {
            return Err(format!("LSP root is not a directory: {}", root.display()));
        }
        Ok(Self {
            executable,
            arguments,
            root,
        })
    }
}

pub struct LspService {
    configs: LspConfigs,
    clients: BTreeMap<Option<Language>, LspClient>,
    failed_starts: BTreeMap<Option<Language>, String>,
}

impl LspService {
    pub fn new(configs: LspConfigs) -> Self {
        Self {
            configs,
            clients: BTreeMap::new(),
            failed_starts: BTreeMap::new(),
        }
    }

    fn client(&mut self, language: Language) -> Result<&mut LspClient, String> {
        let key = self
            .configs
            .languages
            .contains_key(&language)
            .then_some(language);
        if key.is_none() && self.configs.default.is_none() {
            return Err(format!(
                "no LSP server is configured for {}",
                language.name()
            ));
        }
        if let Some(error) = self.failed_starts.get(&key) {
            return Err(error.clone());
        }
        if !self.clients.contains_key(&key) {
            let config = key
                .and_then(|language| self.configs.languages.get(&language))
                .or(self.configs.default.as_ref())
                .expect("an LSP configuration was checked above");
            match LspClient::start(config) {
                Ok(client) => {
                    self.clients.insert(key, client);
                }
                Err(error) => {
                    self.failed_starts.insert(key, error.clone());
                    return Err(error);
                }
            }
        }
        Ok(self
            .clients
            .get_mut(&key)
            .expect("LSP client inserted above"))
    }

    pub fn is_configured(&self, language: Language) -> bool {
        self.configs.languages.contains_key(&language) || self.configs.default.is_some()
    }

    pub fn document_symbols(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
    ) -> Result<Vec<Symbol>, String> {
        self.client(language)?
            .document_symbols(path, language, source)
    }

    pub fn definition(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
    ) -> Result<Vec<LspLocation>, String> {
        self.client(language)?
            .definition(path, language, source, row, byte_column)
    }

    pub fn references(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
        include_declaration: bool,
    ) -> Result<Vec<LspLocation>, String> {
        self.client(language)?.references(
            path,
            language,
            source,
            row,
            byte_column,
            include_declaration,
        )
    }

    pub fn hover(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
    ) -> Result<Option<LspHover>, String> {
        self.client(language)?
            .hover(path, language, source, row, byte_column)
    }
}
