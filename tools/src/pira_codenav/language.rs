use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use tree_sitter::{Language as TsLanguage, Parser};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Language {
    Python,
    Rust,
    Java,
    C,
    Cpp,
    Cuda,
    Bash,
    Go,
    JavaScript,
    TypeScript,
    CSharp,
    PowerShell,
    Php,
    Kotlin,
    Lua,
    Hcl,
    R,
}

impl Language {
    pub const ALL: [Self; 17] = [
        Self::Python,
        Self::Rust,
        Self::Java,
        Self::C,
        Self::Cpp,
        Self::Cuda,
        Self::Bash,
        Self::Go,
        Self::JavaScript,
        Self::TypeScript,
        Self::CSharp,
        Self::PowerShell,
        Self::Php,
        Self::Kotlin,
        Self::Lua,
        Self::Hcl,
        Self::R,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Java => "java",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Cuda => "cuda",
            Self::Bash => "bash",
            Self::Go => "go",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::CSharp => "csharp",
            Self::PowerShell => "powershell",
            Self::Php => "php",
            Self::Kotlin => "kotlin",
            Self::Lua => "lua",
            Self::Hcl => "hcl",
            Self::R => "r",
        }
    }

    pub fn parse_name(value: &str) -> Option<Self> {
        match value {
            "python" => Some(Self::Python),
            "rust" => Some(Self::Rust),
            "java" => Some(Self::Java),
            "c" => Some(Self::C),
            "cpp" | "c++" => Some(Self::Cpp),
            "cuda" => Some(Self::Cuda),
            "bash" | "sh" => Some(Self::Bash),
            "go" | "golang" => Some(Self::Go),
            "javascript" | "js" => Some(Self::JavaScript),
            "typescript" | "ts" | "tsx" => Some(Self::TypeScript),
            "csharp" | "c#" | "cs" => Some(Self::CSharp),
            "powershell" | "pwsh" | "ps" => Some(Self::PowerShell),
            "php" => Some(Self::Php),
            "kotlin" | "kt" | "kts" => Some(Self::Kotlin),
            "lua" => Some(Self::Lua),
            "hcl" | "terraform" | "tf" => Some(Self::Hcl),
            "r" => Some(Self::R),
            _ => None,
        }
    }

    pub fn tree_sitter(self, path: &Path) -> TsLanguage {
        match self {
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Cuda => tree_sitter_cuda::LANGUAGE.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript
                if path
                    .extension()
                    .and_then(OsStr::to_str)
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tsx")) =>
            {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            }
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Self::PowerShell => tree_sitter_powershell::LANGUAGE.into(),
            Self::Php => tree_sitter_php::LANGUAGE_PHP.into(),
            Self::Kotlin => tree_sitter_kotlin_sg::LANGUAGE.into(),
            Self::Lua => tree_sitter_lua::LANGUAGE.into(),
            Self::Hcl => tree_sitter_hcl::LANGUAGE.into(),
            Self::R => tree_sitter_r::LANGUAGE.into(),
        }
    }

    pub fn parser(self, path: &Path) -> Result<Parser, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&self.tree_sitter(path))
            .map_err(|error| format!("failed to initialize {} parser: {error}", self.name()))?;
        Ok(parser)
    }

    pub fn infer(path: &Path) -> Result<Self, String> {
        if let Some(extension) = path.extension().and_then(OsStr::to_str) {
            return match extension.to_ascii_lowercase().as_str() {
                "py" | "pyi" | "pyw" => Ok(Self::Python),
                "rs" => Ok(Self::Rust),
                "java" => Ok(Self::Java),
                "c" => Ok(Self::C),
                "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Ok(Self::Cpp),
                "cu" | "cuh" => Ok(Self::Cuda),
                "sh" | "bash" => Ok(Self::Bash),
                "go" => Ok(Self::Go),
                "js" | "jsx" | "mjs" | "cjs" => Ok(Self::JavaScript),
                "ts" | "tsx" | "mts" | "cts" => Ok(Self::TypeScript),
                "cs" => Ok(Self::CSharp),
                "ps1" | "psm1" | "psd1" => Ok(Self::PowerShell),
                "php" | "php3" | "php4" | "php5" | "phtml" => Ok(Self::Php),
                "kt" | "kts" => Ok(Self::Kotlin),
                "lua" => Ok(Self::Lua),
                "hcl" | "tf" | "tfvars" => Ok(Self::Hcl),
                "r" => Ok(Self::R),
                "h" => Err(format!(
                    "ambiguous C/C++/CUDA header `{}`; rerun with explicit `c`, `cpp`, or `cuda`",
                    path.display()
                )),
                _ => Err(Self::inference_error(path)),
            };
        }
        let mut prefix = Vec::with_capacity(256);
        File::open(path)
            .and_then(|file| file.take(256).read_to_end(&mut prefix))
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let first_line = String::from_utf8_lossy(&prefix)
            .lines()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if first_line.starts_with("#!") && first_line.contains("python") {
            return Ok(Self::Python);
        }
        if first_line.starts_with("#!")
            && (first_line.contains("bash")
                || first_line.ends_with("/sh")
                || first_line.contains("env sh"))
        {
            return Ok(Self::Bash);
        }
        if first_line.starts_with("#!")
            && (first_line.contains("pwsh") || first_line.contains("powershell"))
        {
            return Ok(Self::PowerShell);
        }
        if first_line.starts_with("#!") && first_line.contains("rscript") {
            return Ok(Self::R);
        }
        Err(Self::inference_error(path))
    }

    fn inference_error(path: &Path) -> String {
        format!(
            "cannot determine language for `{}`; rerun with an explicit language, for example: pira_codenav LANGUAGE outline {}",
            path.display(),
            path.display()
        )
    }

    pub fn matches_path(self, path: &Path) -> bool {
        if Self::is_ambiguous_path(path) {
            return matches!(self, Self::C | Self::Cpp | Self::Cuda);
        }
        Self::infer(path).is_ok_and(|detected| detected == self)
    }

    pub fn is_ambiguous_path(path: &Path) -> bool {
        path.extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("h"))
    }
}
