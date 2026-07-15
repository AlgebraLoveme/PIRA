use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::command::{CommandError, input_error};
use crate::language::Language;
use crate::lsp::{LspConfig, LspConfigs};
use crate::util::absolute_lexical;

#[derive(Default)]
struct ServerOptions {
    executable: Option<PathBuf>,
    arguments: Vec<String>,
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
        if server.arguments.is_empty() {
            return Ok(None);
        }
        return Err((
            2,
            format!("--lsp-arg for {label} requires a matching --lsp"),
        ));
    };
    LspConfig::new(executable, server.arguments.clone(), root.to_path_buf())
        .map(Some)
        .map_err(input_error)
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
        if matches!(option, "--lsp" | "--lsp-arg" | "--lsp-root") {
            if !matches!(
                command,
                "outline" | "show" | "map" | "definition" | "references" | "hover"
            ) {
                return Err((
                    2,
                    format!(
                        "{option} is supported only by outline, show, map, definition, references, and hover"
                    ),
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
