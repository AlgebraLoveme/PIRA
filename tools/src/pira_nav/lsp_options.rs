use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::command::{CommandError, input_error};
use crate::language::Language;
use crate::lsp::{LspConfig, LspConfigs, auto_server_available};
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
    native_only: bool,
}

impl LspOptions {
    pub fn native_only(&self) -> bool {
        self.native_only
    }

    pub fn forced_lsp(&self) -> (bool, BTreeSet<Language>) {
        let all = self.default.executable.is_some();
        let languages = self
            .languages
            .iter()
            .filter_map(|(language, server)| server.executable.as_ref().map(|_| *language))
            .collect();
        (all, languages)
    }

    pub fn root<'a>(&'a self, default_root: &'a Path) -> &'a Path {
        self.root.as_deref().unwrap_or(default_root)
    }

    pub fn has_server(&self, language: Language) -> bool {
        self.languages
            .get(&language)
            .and_then(|server| server.executable.as_ref())
            .is_some()
            || self.default.executable.is_some()
            || (self.default.executable.is_none()
                && !self.languages.contains_key(&language)
                && auto_server_available(language))
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
        let auto_root = default.is_none().then(|| root.clone());
        Ok(LspConfigs {
            default,
            languages,
            auto_root,
        })
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
    let file = File::open(path).map_err(|error| {
        (
            2,
            format!("cannot open {option} file {}: {error}", path.display()),
        )
    })?;
    let metadata = file.metadata().map_err(|error| {
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
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_LSP_CONFIG_BYTES) as usize);
    file.take(MAX_LSP_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            (
                2,
                format!("cannot read {option} file {}: {error}", path.display()),
            )
        })?;
    if bytes.len() as u64 > MAX_LSP_CONFIG_BYTES {
        return Err((
            2,
            format!("{option} file exceeds 64 KiB: {}", path.display()),
        ));
    }
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
        if option == "--" {
            remaining.extend(args[index..].iter().cloned());
            break;
        } else if matches!(
            option,
            "--lsp" | "--lsp-arg" | "--lsp-root" | "--lsp-init" | "--lsp-settings"
        ) {
            if !supports_lsp(command) {
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
        } else if option == "--native" {
            if !supports_native_structural(command) {
                return Err((
                    2,
                    "--native is supported only by outline, show, map, and symbols".into(),
                ));
            }
            if options.native_only {
                return Err((2, "--native may be specified only once".into()));
            }
            options.native_only = true;
            index += 1;
        } else {
            remaining.push(args[index].clone());
            index += 1;
        }
    }
    if options.native_only {
        let has_lsp_configuration = options.default.executable.is_some()
            || !options.default.arguments.is_empty()
            || options.default.initialization.is_some()
            || options.default.settings.is_some()
            || !options.languages.is_empty()
            || options.root.is_some();
        if has_lsp_configuration {
            return Err((
                2,
                "--native cannot be combined with LSP configuration".into(),
            ));
        }
    }
    Ok((remaining, options))
}

fn supports_native_structural(command: &str) -> bool {
    matches!(command, "outline" | "show" | "map" | "symbols")
}

fn supports_lsp(command: &str) -> bool {
    matches!(
        command,
        "outline"
            | "show"
            | "map"
            | "symbols"
            | "definition"
            | "implementation"
            | "type-definition"
            | "references"
            | "hover"
            | "callers"
            | "callees"
            | "supertypes"
            | "subtypes"
            | "query"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::read_json;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn lsp_json_read_is_bounded_by_the_open_handle() {
        let path = std::env::temp_dir().join(format!(
            "pira-nav-lsp-config-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, vec![b' '; 64 * 1024 + 1]).unwrap();
        let error = read_json(&path, "--lsp-init").unwrap_err();
        assert!(error.1.contains("exceeds 64 KiB"));
        fs::remove_file(path).unwrap();
    }
}
