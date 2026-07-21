use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::language::Language;
use crate::util::absolute_lexical;

pub struct FileDiscovery {
    pub files: Vec<(PathBuf, Language)>,
    pub discovered: usize,
    pub unsupported: usize,
    pub ambiguous: usize,
}

#[derive(Clone, Copy)]
pub enum DiscoverySelection {
    Any,
    Exact(Language),
    Dependencies(Language),
}

enum DiscoveredLanguage {
    Eligible(Language),
    Unsupported,
    Ambiguous,
}

fn dependency_languages_are_compatible(target: Language, candidate: Language) -> bool {
    target == candidate
        || matches!(
            (target, candidate),
            (
                Language::C | Language::Cpp | Language::Cuda,
                Language::C | Language::Cpp | Language::Cuda
            ) | (
                Language::JavaScript | Language::TypeScript,
                Language::JavaScript | Language::TypeScript
            )
        )
}

fn classify(path: &Path, selection: DiscoverySelection) -> DiscoveredLanguage {
    match selection {
        DiscoverySelection::Any => {
            if Language::is_ambiguous_path(path) {
                DiscoveredLanguage::Ambiguous
            } else {
                Language::infer(path)
                    .map(DiscoveredLanguage::Eligible)
                    .unwrap_or(DiscoveredLanguage::Unsupported)
            }
        }
        DiscoverySelection::Exact(language) => {
            if language.matches_path(path) {
                DiscoveredLanguage::Eligible(language)
            } else {
                DiscoveredLanguage::Unsupported
            }
        }
        DiscoverySelection::Dependencies(target) => {
            if Language::is_ambiguous_path(path) {
                if matches!(target, Language::C | Language::Cpp | Language::Cuda) {
                    DiscoveredLanguage::Eligible(target)
                } else {
                    DiscoveredLanguage::Unsupported
                }
            } else {
                match Language::infer(path) {
                    Ok(candidate) if dependency_languages_are_compatible(target, candidate) => {
                        DiscoveredLanguage::Eligible(candidate)
                    }
                    _ => DiscoveredLanguage::Unsupported,
                }
            }
        }
    }
}

pub fn discover_files(root: &Path, selection: DiscoverySelection) -> FileDiscovery {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .require_git(false)
        .follow_links(false);
    let mut files = Vec::new();
    let mut discovered = 0;
    let mut unsupported = 0;
    let mut ambiguous = 0;
    for entry in builder.build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        discovered += 1;
        let path = absolute_lexical(entry.path(), root);
        match classify(&path, selection) {
            DiscoveredLanguage::Eligible(language) => files.push((path, language)),
            DiscoveredLanguage::Unsupported => unsupported += 1,
            DiscoveredLanguage::Ambiguous => ambiguous += 1,
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    FileDiscovery {
        files,
        discovered,
        unsupported,
        ambiguous,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_compatibility_is_narrow_and_symmetric() {
        assert!(dependency_languages_are_compatible(
            Language::C,
            Language::Cuda
        ));
        assert!(dependency_languages_are_compatible(
            Language::TypeScript,
            Language::JavaScript
        ));
        assert!(!dependency_languages_are_compatible(
            Language::Python,
            Language::TypeScript
        ));
    }
}
