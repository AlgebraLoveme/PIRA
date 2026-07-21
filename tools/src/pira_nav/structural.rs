use std::collections::BTreeSet;
use std::path::Path;

use crate::command::{CommandError, input_error, lsp_error};
use crate::language::Language;
use crate::lsp::{LspConfigs, LspService};
use crate::model::ParseBackend;
use crate::parse::{ParsedFile, parse_file, parse_source_symbols};

pub struct StructuralResolver {
    service: LspService,
    native_only: bool,
    force_all_lsp: bool,
    forced_languages: BTreeSet<Language>,
}

impl StructuralResolver {
    pub fn new(
        configs: LspConfigs,
        native_only: bool,
        force_all_lsp: bool,
        forced_languages: BTreeSet<Language>,
    ) -> Self {
        Self {
            service: LspService::new(configs),
            native_only,
            force_all_lsp,
            forced_languages,
        }
    }

    pub fn lsp_only(configs: LspConfigs) -> Self {
        Self::new(configs, false, true, BTreeSet::new())
    }

    pub fn resolve_path(
        &mut self,
        path: &Path,
        language: Language,
    ) -> Result<ParsedFile, CommandError> {
        let parsed = parse_file(path, language).map_err(input_error)?;
        self.resolve_parsed(parsed)
    }

    pub fn resolve_parsed(&mut self, parsed: ParsedFile) -> Result<ParsedFile, CommandError> {
        let force_lsp = self.force_all_lsp || self.forced_languages.contains(&parsed.language);
        if self.native_only || (!force_lsp && parsed.syntax_defects == 0) {
            return self.resolve_native(parsed);
        }
        let path = parsed.path.clone();
        let language = parsed.language;
        if !self.service.is_configured(language) {
            let message = if language.is_document() {
                format!(
                    "syntax-dirty {} document requires an explicit server via --lsp {}=ABSOLUTE_SERVER_PATH; otherwise use search or an exact show line range",
                    language.name(),
                    language.name()
                )
            } else {
                format!(
                    "syntax-dirty {} source requires an LSP; install a conventional server on PATH or pass --lsp {}=ABSOLUTE_SERVER_PATH",
                    language.name(),
                    language.name()
                )
            };
            return Err(lsp_error(message));
        }
        let source = parsed.source;
        let mut symbols = self
            .service
            .document_symbols(&path, language, &source)
            .map_err(lsp_error)?;
        if let Ok((native_symbols, 0)) = parse_source_symbols(&path, language, &source) {
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
            path,
            language,
            source,
            symbols,
            backend: ParseBackend::Lsp,
            syntax_defects: 0,
            symbols_truncated: false,
        })
    }

    pub fn resolve_native(&self, parsed: ParsedFile) -> Result<ParsedFile, CommandError> {
        if parsed.syntax_defects > 0 {
            let recovery = if parsed.language.is_document() {
                format!(
                    "pass --lsp {}=ABSOLUTE_SERVER_PATH, or use search or an exact show line range",
                    parsed.language.name()
                )
            } else {
                format!(
                    "rerun without --native with a conventional {} LSP on PATH or pass --lsp {}=ABSOLUTE_SERVER_PATH",
                    parsed.language.name(),
                    parsed.language.name()
                )
            };
            return Err(lsp_error(format!(
                "native parser found {} syntax defect(s); {recovery}",
                parsed.syntax_defects
            )));
        }
        Ok(parsed)
    }
}
