//! Ignore loading is deliberately separate from traversal: WalkBuilder's automatic
//! config/ignore readers cannot enforce Nav's no-follow and resource boundaries.
use crate::util::{read_source, reject_symlink_components};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_IGNORE_LINE_BYTES: usize = 64 * 1024;
const MAX_IGNORE_LINES: usize = 100_000;

#[derive(Default)]
pub(crate) struct WalkFiles {
    pub paths: Vec<PathBuf>,
    pub errors: Vec<String>,
    pub errors_total: usize,
}
impl WalkFiles {
    fn error(&mut self, error: impl ToString) {
        self.errors_total += 1;
        if self.errors.len() < 20 {
            self.errors.push(error.to_string());
        }
    }
    fn text(&mut self, path: &Path) -> Option<String> {
        // Missing optional files are normal; symlinks (including dangling ones),
        // special files, unreadable files and invalid text are coverage failures.
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                self.error(format!("{}: {e}", path.display()));
                return None;
            }
            Ok(_) => {}
        }
        match read_source(path) {
            Ok(text) => {
                if text.lines().take(MAX_IGNORE_LINES + 1).count() > MAX_IGNORE_LINES
                    || text.lines().any(|line| line.len() > MAX_IGNORE_LINE_BYTES)
                {
                    self.error(format!(
                        "ignore/config line safety limit exceeded: {}",
                        path.display()
                    ));
                    None
                } else {
                    Some(text)
                }
            }
            Err(e) => {
                self.error(e);
                None
            }
        }
    }
    fn matcher(&mut self, root: &Path, path: &Path) -> Gitignore {
        let mut builder = GitignoreBuilder::new(root);
        if let Some(text) = self.text(path) {
            for (index, line) in text.lines().enumerate() {
                let line = if index == 0 {
                    line.trim_start_matches('\u{feff}')
                } else {
                    line
                };
                if let Err(e) = builder.add_line(Some(path.to_path_buf()), line) {
                    self.error(format!("{}:{}: {e}", path.display(), index + 1));
                }
            }
        }
        match builder.build() {
            Ok(matcher) => matcher,
            Err(e) => {
                self.error(e);
                Gitignore::empty()
            }
        }
    }
}

struct Rules {
    parent: Option<Arc<Rules>>,
    ignore: Gitignore,
    gitignore: Gitignore,
    exclude: Gitignore,
}
impl Rules {
    fn matched(&self, path: &Path, is_dir: bool, global: &Gitignore) -> Option<bool> {
        let mut current = Some(self);
        let (mut ordinary, mut git, mut exclude) = (None, None, None);
        while let Some(rules) = current {
            let matched = |matcher: &Gitignore, slot: &mut Option<bool>| {
                if slot.is_none() {
                    let m = matcher.matched(path, is_dir);
                    if !m.is_none() {
                        *slot = Some(m.is_ignore());
                    }
                }
            };
            matched(&rules.ignore, &mut ordinary);
            matched(&rules.gitignore, &mut git);
            matched(&rules.exclude, &mut exclude);
            current = rules.parent.as_deref();
        }
        ordinary.or(git).or(exclude).or_else(|| {
            let m = global.matched(path, is_dir);
            (!m.is_none()).then(|| m.is_ignore())
        })
    }
}

fn directory_rules(path: &Path, parent: Option<Arc<Rules>>, out: &mut WalkFiles) -> Arc<Rules> {
    let git = path.join(".git");
    // Nav historically uses require_git(false): rules continue across nested
    // repositories, and excludes come only from .git directories, not worktree
    // pointer files. Do not broaden the set of config paths read here.
    let exclude = match std::fs::symlink_metadata(&git) {
        Ok(metadata) if metadata.is_dir() => out.matcher(path, &git.join("info/exclude")),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            out.error(format!(
                "pira_nav does not follow symlinks: {}",
                git.display()
            ));
            Gitignore::empty()
        }
        Ok(_) => Gitignore::empty(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Gitignore::empty(),
        Err(e) => {
            out.error(format!("{}: {e}", git.display()));
            Gitignore::empty()
        }
    };
    Arc::new(Rules {
        parent,
        ignore: out.matcher(path, &path.join(".ignore")),
        gitignore: out.matcher(path, &path.join(".gitignore")),
        exclude,
    })
}

fn global_rules(root: &Path, out: &mut WalkFiles) -> Gitignore {
    #[allow(deprecated)]
    let home = std::env::home_dir();
    let env_path = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let xdg = env_path("XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|p| p.join(".config")));
    // Match ignore's existing precedence and excludesFile extraction, but never
    // delegate config reads to its unrestricted reader.
    let configs = [
        env_path("GIT_CONFIG_GLOBAL"),
        home.as_ref().map(|p| p.join(".gitconfig")),
        xdg.as_ref().map(|p| p.join("git/config")),
        Some(env_path("GIT_CONFIG_SYSTEM").unwrap_or_else(|| PathBuf::from("/etc/gitconfig"))),
    ];
    let re = regex::bytes::Regex::new(r#"(?im-u)^\s*excludesfile\s*=\s*"?\s*(\S+?)\s*"?\s*$"#);
    let mut selected = None;
    let re = match re {
        Ok(re) => re,
        Err(e) => {
            out.error(e);
            return Gitignore::empty();
        }
    };
    {
        for config in configs.into_iter().flatten() {
            if let Some(text) = out.text(&config) {
                if let Some(caps) = re.captures(text.as_bytes()) {
                    let Ok(value) = std::str::from_utf8(&caps[1]) else {
                        continue;
                    };
                    selected = Some(PathBuf::from(match &home {
                        Some(home) => value.replace('~', &home.to_string_lossy()),
                        None => value.to_string(),
                    }));
                    break;
                }
            }
        }
    }
    let selected = selected.or_else(|| xdg.map(|p| p.join("git/ignore")));
    selected
        .map(|p| out.matcher(root, &p))
        .unwrap_or_else(Gitignore::empty)
}

pub(crate) fn files(root: &Path, max_depth: Option<usize>) -> WalkFiles {
    let mut out = WalkFiles::default();
    if let Err(e) = reject_symlink_components(root) {
        out.error(e);
        return out;
    }
    if root.is_file() {
        out.paths.push(root.to_path_buf());
        return out;
    }
    let global = global_rules(root, &mut out);
    let mut parent = None;
    for ancestor in root
        .ancestors()
        .skip(1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        parent = Some(directory_rules(ancestor, parent, &mut out));
    }
    let mut pending = vec![(root.to_path_buf(), parent, 0)];
    while let Some((dir, parent, depth)) = pending.pop() {
        if crate::cancellation::requested() {
            out.error("discovery cancelled");
            break;
        }
        if let Err(e) = reject_symlink_components(&dir) {
            out.error(e);
            continue;
        }
        if max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        let rules = directory_rules(&dir, parent, &mut out);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                out.error(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    out.error(e);
                    continue;
                }
            };
            let kind = match entry.file_type() {
                Ok(k) => k,
                Err(e) => {
                    out.error(e);
                    continue;
                }
            };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            // Whitelisting can expose a hidden path, just as in ignore::Walk.
            let matched = rules.matched(&path, kind.is_dir(), &global);
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            #[cfg(windows)]
            let hidden = {
                use std::os::windows::fs::MetadataExt;
                match entry.metadata() {
                    Ok(metadata) => hidden || metadata.file_attributes() & 2 != 0,
                    Err(e) => {
                        out.error(e);
                        continue;
                    }
                }
            };
            if matched == Some(true) || (hidden && matched != Some(false)) {
                continue;
            }
            if kind.is_file() {
                out.paths.push(path);
            } else if kind.is_dir() {
                pending.push((path, Some(Arc::clone(&rules)), depth + 1));
            }
        }
    }
    out.paths.sort();
    out
}
