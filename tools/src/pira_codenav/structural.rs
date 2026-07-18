use crate::lsp::{LspConfigs, LspService};
use crate::model::ParseBackend;
use crate::parse::ParsedFile;

pub struct StructuralResolver {
    service: LspService,
}

impl StructuralResolver {
    pub fn new(configs: LspConfigs) -> Self {
        Self {
            service: LspService::new(configs),
        }
    }

    pub fn resolve(&mut self, mut parsed: ParsedFile) -> Result<ParsedFile, String> {
        if parsed.syntax_defects == 0 {
            return Ok(parsed);
        }
        if !self.service.is_configured(parsed.language) {
            return Err(format!(
                "Tree-sitter found {} syntax defect(s) in {}; install a conventional {} LSP on PATH or pass --lsp {}=ABSOLUTE_SERVER_PATH to obtain a clean structural result",
                parsed.syntax_defects,
                parsed.path.display(),
                parsed.language.name(),
                parsed.language.name()
            ));
        }
        parsed.symbols =
            self.service
                .document_symbols(&parsed.path, parsed.language, &parsed.source)?;
        parsed.backend = ParseBackend::Lsp;
        parsed.syntax_defects = 0;
        Ok(parsed)
    }
}
