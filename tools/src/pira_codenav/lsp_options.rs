use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::command::{CommandError, input_error};
use crate::language::Language;
use crate::lsp::{LspConfig, LspConfigs};
use crate::util::absolute_lexical;

const MAX_LSP_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Default)]
struct ServerOptions {
    executable: Option<PathBuf>,
    arguments: Vec<String>,
    initialization: Option<PathBuf>,
    settings: Option<PathBuf>,
}

#[derive(Default)]
pub struct LspOptions {
    default: ServerOptions,
    languages: BTreeMap<Language, ServerOptions>,
    root: Option<PathBuf>,
}

impl LspOptions {
    pub fn has_server(&self, language: Language) -> bool {
        self.languages
            .get(&language)
            .and_then(|server| server.executable.as_ref())
            .is_some()
            || self.default.executable.is_some()
    }

    pub fn config(&self, default_root: &Path) -> Result<LspConfigs, CommandError> {
        let root = self
            .root
            .clone()
            .unwrap_or_else(|| default_root.to_path_buf());
        let default = build_config(&self.default, &root, "default")?;
        let mut languages = BTreeMap::new();
        for (language, server) in &self.languages {
            if let Some(config) = build_config(server, &root, language.name())? {
                languages.insert(*language, config);
            }
        }
        if self.root.is_some() && default.is_none() && languages.is_empty() {
            return Err((2, "--lsp-root requires at least one --lsp".into()));
        }
        Ok(LspConfigs { default, languages })
    }
}

fn build_config(
    server: &ServerOptions,
    root: &Path,
    label: &str,
) -> Result<Option<LspConfig>, CommandError> {
    let Some(executable) = server.executable.clone() else {
        if server.arguments.is_empty()
            && server.initialization.is_none()
            && server.settings.is_none()
        {
            return Ok(None);
        }
        return Err((
            2,
            format!("LSP configuration for {label} requires a matching --lsp"),
        ));
    };
    let initialization = server
        .initialization
        .as_deref()
        .map(|path| read_json(path, "--lsp-init"))
        .transpose()?;
    let settings = server
        .settings
        .as_deref()
        .map(|path| read_json(path, "--lsp-settings"))
        .transpose()?;
    LspConfig::new(
        executable,
        server.arguments.clone(),
        root.to_path_buf(),
        initialization,
        settings,
    )
    .map(Some)
    .map_err(input_error)
}

fn read_json(path: &Path, option: &str) -> Result<Value, CommandError> {
    let metadata = fs::metadata(path).map_err(|error| {
        (
            2,
            format!("cannot inspect {option} file {}: {error}", path.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err((
            2,
            format!("{option} path is not a regular file: {}", path.display()),
        ));
    }
    if metadata.len() > MAX_LSP_CONFIG_BYTES {
        return Err((
            2,
            format!("{option} file exceeds 64 KiB: {}", path.display()),
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        (
            2,
            format!("cannot read {option} file {}: {error}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        (
            2,
            format!("invalid JSON in {option} file {}: {error}", path.display()),
        )
    })
}

fn language_assignment(value: &str) -> Option<(Language, &str)> {
    let (name, assigned) = value.split_once('=')?;
    Language::parse_name(name).map(|language| (language, assigned))
}

pub fn parse(
    args: &[String],
    command: &str,
    cwd: &Path,
) -> Result<(Vec<String>, LspOptions), CommandError> {
    let mut remaining = Vec::with_capacity(args.len());
    let mut options = LspOptions::default();
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if matches!(
            option,
            "--lsp" | "--lsp-arg" | "--lsp-root" | "--lsp-init" | "--lsp-settings"
        ) {
            if !matches!(
                command,
                "outline"
                    | "show"
                    | "map"
                    | "find"
                    | "definition"
                    | "implementation"
                    | "type-definition"
                    | "references"
                    | "hover"
                    | "callers"
                    | "callees"
            ) {
                return Err((
                    2,
                    format!("{option} is supported only by structural/LSP navigation commands"),
                ));
            }
            let value = args
                .get(index + 1)
                .ok_or_else(|| (2, format!("{option} requires a value")))?;
            match option {
                "--lsp" => {
                    let (server, path, label) =
                        if let Some((language, path)) = language_assignment(value) {
                            (
                                options.languages.entry(language).or_default(),
                                path,
                                language.name(),
                            )
                        } else {
                            (&mut options.default, value.as_str(), "default")
                        };
                    if path.is_empty() {
                        return Err((2, format!("--lsp path for {label} must not be empty")));
                    }
                    if server.executable.replace(PathBuf::from(path)).is_some() {
                        return Err((2, format!("--lsp for {label} may be specified only once")));
                    }
                }
                "--lsp-arg" => {
                    if let Some((language, argument)) = language_assignment(value) {
                        options
                            .languages
                            .entry(language)
                            .or_default()
                            .arguments
                            .push(argument.to_string());
                    } else {
                        options.default.arguments.push(value.clone());
                    }
                }
                "--lsp-init" | "--lsp-settings" => {
                    let (server, path, label) =
                        if let Some((language, path)) = language_assignment(value) {
                            (
                                options.languages.entry(language).or_default(),
                                path,
                                language.name(),
                            )
                        } else {
                            (&mut options.default, value.as_str(), "default")
                        };
                    if path.is_empty() {
                        return Err((2, format!("{option} path for {label} must not be empty")));
                    }
                    let slot = if option == "--lsp-init" {
                        &mut server.initialization
                    } else {
                        &mut server.settings
                    };
                    if slot
                        .replace(absolute_lexical(Path::new(path), cwd))
                        .is_some()
                    {
                        return Err((
                            2,
                            format!("{option} for {label} may be specified only once"),
                        ));
                    }
                }
                "--lsp-root" => {
                    if options
                        .root
                        .replace(absolute_lexical(Path::new(value), cwd))
                        .is_some()
                    {
                        return Err((2, "--lsp-root may be specified only once".into()));
                    }
                }
                _ => unreachable!(),
            }
            index += 2;
        } else {
            remaining.push(args[index].clone());
            index += 1;
        }
    }
    Ok((remaining, options))
}
