use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::language::Language;
use crate::model::Symbol;

mod client;
mod protocol;

use client::LspClient;
pub use protocol::{
    LspCall, LspHover, LspLocation, LspRange, LspTypeItem, PositionEncoding, file_path_from_uri,
    normalize_range,
};

#[derive(Clone, Debug)]
pub struct LspConfig {
    executable: PathBuf,
    arguments: Vec<String>,
    root: PathBuf,
    initialization_options: Option<Value>,
    settings: Option<Value>,
}

#[derive(Default)]
pub struct LspConfigs {
    pub default: Option<LspConfig>,
    pub languages: BTreeMap<Language, LspConfig>,
    pub auto_root: Option<PathBuf>,
}

impl LspConfig {
    pub fn new(
        executable: PathBuf,
        arguments: Vec<String>,
        root: PathBuf,
        initialization_options: Option<Value>,
        settings: Option<Value>,
    ) -> Result<Self, String> {
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
            initialization_options,
            settings,
        })
    }
}

pub struct LspService {
    configs: LspConfigs,
    clients: BTreeMap<Option<Language>, LspClient>,
    failed_starts: BTreeMap<Option<Language>, String>,
    retain_documents: bool,
}

impl LspService {
    pub fn new(configs: LspConfigs) -> Self {
        Self::with_document_reuse(configs, false)
    }

    pub fn new_semantic(configs: LspConfigs) -> Self {
        Self::with_document_reuse(configs, true)
    }

    fn with_document_reuse(configs: LspConfigs, retain_documents: bool) -> Self {
        Self {
            configs,
            clients: BTreeMap::new(),
            failed_starts: BTreeMap::new(),
            retain_documents,
        }
    }

    fn client(&mut self, language: Language) -> Result<&mut LspClient, String> {
        if !self.configs.languages.contains_key(&language)
            && self.configs.default.is_none()
            && let Some(root) = self.configs.auto_root.as_deref()
            && let Some(config) = discover_config(language, root)?
        {
            self.configs.languages.insert(language, config);
        }
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
            match LspClient::start(config, self.retain_documents) {
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
        self.configs.languages.contains_key(&language)
            || self.configs.default.is_some()
            || self
                .configs
                .auto_root
                .as_deref()
                .is_some_and(|_| auto_server_available(language))
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

    pub fn implementation(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
    ) -> Result<Vec<LspLocation>, String> {
        self.client(language)?
            .implementation(path, language, source, row, byte_column)
    }

    pub fn type_definition(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
    ) -> Result<Vec<LspLocation>, String> {
        self.client(language)?
            .type_definition(path, language, source, row, byte_column)
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

    pub fn calls(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
        incoming: bool,
    ) -> Result<Vec<LspCall>, String> {
        self.client(language)?
            .calls(path, language, source, row, byte_column, incoming)
    }

    pub fn type_hierarchy(
        &mut self,
        path: &Path,
        language: Language,
        source: &str,
        row: usize,
        byte_column: usize,
        supertypes: bool,
    ) -> Result<Vec<LspTypeItem>, String> {
        self.client(language)?
            .type_hierarchy(path, language, source, row, byte_column, supertypes)
    }
}

pub fn auto_server_available(language: Language) -> bool {
    discover_executable(language).is_some()
}

pub fn auto_server_name(language: Language) -> Option<String> {
    discover_executable(language).and_then(|(path, _)| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    })
}

fn discover_config(language: Language, root: &Path) -> Result<Option<LspConfig>, String> {
    let Some((executable, arguments)) = discover_executable(language) else {
        return Ok(None);
    };
    LspConfig::new(executable, arguments, root.to_path_buf(), None, None).map(Some)
}

fn discover_executable(language: Language) -> Option<(PathBuf, Vec<String>)> {
    let candidates: &[(&str, &[&str])] = match language {
        Language::Python => &[
            ("basedpyright-langserver", &["--stdio"]),
            ("pyright-langserver", &["--stdio"]),
            ("pylsp", &[]),
        ],
        Language::Rust => &[("rust-analyzer", &[])],
        Language::Java => &[("jdtls", &[])],
        Language::C | Language::Cpp | Language::Cuda => &[("clangd", &[])],
        Language::Bash => &[("bash-language-server", &["start"])],
        Language::Go => &[("gopls", &[])],
        Language::JavaScript | Language::TypeScript => {
            &[("typescript-language-server", &["--stdio"])]
        }
        Language::CSharp => &[("csharp-ls", &[])],
        Language::Php => &[("intelephense", &["--stdio"])],
        Language::Kotlin => &[("kotlin-language-server", &[])],
        Language::Lua => &[("lua-language-server", &[])],
        Language::Hcl => &[("terraform-ls", &["serve"])],
        Language::Ruby => &[("solargraph", &["stdio"])],
        Language::Swift => &[("sourcekit-lsp", &[])],
        Language::Scala => &[("metals", &[])],
        Language::PowerShell
        | Language::R
        | Language::Dart
        | Language::Elixir
        | Language::Julia
        | Language::Json
        | Language::Jsonc
        | Language::Yaml
        | Language::Toml
        | Language::Markdown => &[],
    };
    candidates.iter().find_map(|(name, arguments)| {
        executable_in_path(name).map(|executable| {
            (
                executable,
                arguments.iter().map(|value| (*value).to_string()).collect(),
            )
        })
    })
}

fn executable_in_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let names = executable_names(name);
    env::split_paths(&path)
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find_map(|candidate| {
            is_executable_file(&candidate)
                .then(|| fs::canonicalize(candidate).ok())
                .flatten()
        })
}

#[cfg(windows)]
fn executable_names(name: &str) -> Vec<String> {
    if Path::new(name).extension().is_some() {
        return vec![name.to_string()];
    }
    let extensions = env::var_os("PATHEXT")
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    extensions
        .split(';')
        .filter(|extension| !extension.is_empty())
        .map(|extension| format!("{name}{extension}"))
        .collect()
}

#[cfg(not(windows))]
fn executable_names(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}
