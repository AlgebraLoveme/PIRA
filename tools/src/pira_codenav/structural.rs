use std::collections::BTreeSet;
use std::path::Path;

use crate::command::{CommandError, input_error, lsp_error};
use crate::language::Language;
use crate::lsp::{LspConfigs, LspService};
use crate::model::ParseBackend;
use crate::parse::{ParsedFile, parse_file, parse_source_symbols};
use crate::util::read_source;

pub struct StructuralResolver {
    service: LspService,
    native_only: bool,
}

impl StructuralResolver {
    pub fn new(configs: LspConfigs, native_only: bool) -> Self {
        Self {
            service: LspService::new(configs),
            native_only,
        }
    }

    pub fn native_only(&self) -> bool {
        self.native_only
    }

    pub fn require_languages<I>(&self, languages: I) -> Result<(), CommandError>
    where
        I: IntoIterator<Item = Language>,
    {
        if self.native_only {
            return Ok(());
        }
        let missing = languages
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|language| !self.service.is_configured(*language))
            .map(|language| language.name())
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(());
        }
        Err(lsp_error(format!(
            "warning: no LSP server was found for {}; install a conventional server on PATH, pass --lsp [LANGUAGE=]ABSOLUTE_SERVER_PATH, or explicitly choose native parsing with --no-lsp",
            missing.join(",")
        )))
    }

    pub fn resolve_path(
        &mut self,
        path: &Path,
        language: Language,
    ) -> Result<ParsedFile, CommandError> {
        if self.native_only {
            return self.resolve_native(parse_file(path, language).map_err(input_error)?);
        }
        let source = read_source(path).map_err(input_error)?;
        let mut symbols = self
            .service
            .document_symbols(path, language, &source)
            .map_err(lsp_error)?;
        if let Ok((native_symbols, 0)) = parse_source_symbols(path, language, &source) {
            let lsp_names = symbols
                .iter()
                .map(|symbol| symbol.qualified_name.clone())
                .collect::<BTreeSet<_>>();
            symbols.extend(
                native_symbols
                    .into_iter()
                    .filter(|symbol| !lsp_names.contains(&symbol.qualified_name)),
            );
            symbols.sort_by_key(|symbol| (symbol.start_byte, symbol.end_byte));
        }
        Ok(ParsedFile {
            path: path.to_path_buf(),
            language,
            source,
            symbols,
            backend: ParseBackend::Lsp,
            syntax_defects: 0,
        })
    }

    pub fn resolve_native(&self, parsed: ParsedFile) -> Result<ParsedFile, CommandError> {
        if parsed.syntax_defects > 0 {
            return Err(lsp_error(format!(
                "Tree-sitter found {} syntax defect(s) in {}; native parsing was explicitly selected with --no-lsp, so rerun without --no-lsp with a conventional {} LSP on PATH or pass --lsp {}=ABSOLUTE_SERVER_PATH",
                parsed.syntax_defects,
                parsed.path.display(),
                parsed.language.name(),
                parsed.language.name()
            )));
        }
        Ok(parsed)
    }
}
