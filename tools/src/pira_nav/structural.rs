use std::collections::BTreeSet;
use std::path::Path;

use crate::command::{CommandError, input_error, lsp_error};
use crate::language::Language;
use crate::lsp::{LspConfigs, LspService};
use crate::model::ParseBackend;
use crate::parse::{ParsedFile, parse_file, parse_file_source};
use crate::util::read_source;

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

    /// Load a forced-server input without invoking the native parser.
    pub fn load_for_lsp(path: &Path, language: Language) -> Result<ParsedFile, String> {
        Ok(ParsedFile {
            path: path.to_path_buf(),
            language,
            source: read_source(path)?,
            symbols: Vec::new(),
            backend: ParseBackend::Lsp,
            syntax_defects: 0,
            symbols_truncated: false,
        })
    }

    pub fn resolve_path(
        &mut self,
        path: &Path,
        language: Language,
    ) -> Result<ParsedFile, CommandError> {
        let parsed = if self.force_all_lsp || self.forced_languages.contains(&language) {
            Self::load_for_lsp(path, language)
        } else {
            parse_file(path, language)
        }
        .map_err(input_error)?;
        self.resolve_parsed(parsed)
    }

    /// Semantic target preparation retains discovery for dirty source identity.
    pub fn resolve_semantic_source(
        &mut self,
        path: &Path,
        language: Language,
        source: String,
    ) -> Result<ParsedFile, CommandError> {
        let parsed = if self.force_all_lsp || self.forced_languages.contains(&language) {
            ParsedFile {
                path: path.to_path_buf(),
                language,
                source,
                symbols: Vec::new(),
                backend: ParseBackend::Lsp,
                syntax_defects: 0,
                symbols_truncated: false,
            }
        } else {
            parse_file_source(path, language, source).map_err(input_error)?
        };
        if !self.native_only && parsed.syntax_defects > 0 {
            return self.resolve_lsp(parsed);
        }
        self.resolve_parsed(parsed)
    }

    pub fn resolve_parsed(&mut self, parsed: ParsedFile) -> Result<ParsedFile, CommandError> {
        let force_lsp = self.force_all_lsp || self.forced_languages.contains(&parsed.language);
        if self.native_only || !force_lsp {
            return self.resolve_native(parsed);
        }
        self.resolve_lsp(parsed)
    }

    fn resolve_lsp(&mut self, parsed: ParsedFile) -> Result<ParsedFile, CommandError> {
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
        let symbols = self
            .service
            .document_symbols(&path, language, &source)
            .map_err(lsp_error)?;
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
            let recovery = format!(
                "use search or an exact show line range, or explicitly select --lsp {}=ABSOLUTE_SERVER_PATH for a server inventory",
                parsed.language.name()
            );
            return Err(lsp_error(format!(
                "native parser found {} syntax defect(s); {recovery}",
                parsed.syntax_defects
            )));
        }
        Ok(parsed)
    }
}
