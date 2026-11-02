#![cfg(unix)]
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "nav-inputs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("scope/sub")).unwrap();
        fs::create_dir_all(path.join("home")).unwrap();
        let f = Self(path.canonicalize().unwrap());
        fs::write(f.0.join("system.config"), "").unwrap();
        fs::write(f.0.join("scope/keep.py"), "def keep(): pass\n").unwrap();
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        let start = std::time::Instant::now();
        let mut child = Command::new(env!("CARGO_BIN_EXE_pira_nav"))
            .args(args)
            .current_dir(self.0.join("scope"))
            .env("HOME", self.0.join("home"))
            .env("XDG_CONFIG_HOME", self.0.join("home"))
            .env("GIT_CONFIG_GLOBAL", self.0.join("system.config"))
            .env("GIT_CONFIG_SYSTEM", self.0.join("system.config"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > std::time::Duration::from_secs(6) {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("bounded guarded-input test timed out: {args:?}: {output:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn partial(&self) {
        for args in [
            vec!["search", "keep", "."],
            vec!["map", ".", "--native"],
            vec!["symbols", "keep", ".", "--native"],
        ] {
            let out = self.run(&args);
            let text = String::from_utf8(out.stdout).unwrap();
            assert!(
                text.contains("complete=0"),
                "{args:?}: {text} {:?}",
                out.stderr
            );
            assert!(text.contains("keep.py"), "{text}");
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ignore_symlink_is_not_followed_and_reports_partial_in_all_discovery_commands() {
    let f = Fixture::new();
    fs::write(f.0.join("outside.rules"), "keep.py\n").unwrap();
    std::os::unix::fs::symlink("../outside.rules", f.0.join("scope/.gitignore")).unwrap();
    f.partial();
}

#[test]
fn ignore_fifo_is_rejected_without_a_writer() {
    let f = Fixture::new();
    let path = std::ffi::CString::new(f.0.join("scope/.ignore").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    f.partial();
}

#[test]
fn malformed_ignore_rules_report_partial_and_keep_valid_rules() {
    let f = Fixture::new();
    fs::write(f.0.join("scope/.ignore"), "{unclosed\nhidden.py\n").unwrap();
    fs::write(f.0.join("scope/hidden.py"), "def hidden(): pass\n").unwrap();
    f.partial();
    let out = f.run(&["search", "hidden", "."]);
    assert!(!String::from_utf8(out.stdout).unwrap().contains("hidden.py"));
}

#[test]
fn oversized_ignore_bytes_and_lines_are_bounded() {
    let f = Fixture::new();
    let path = f.0.join("scope/.ignore");
    let file = fs::File::create(&path).unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    f.partial();
    fs::write(&path, "x".repeat(64 * 1024 + 1)).unwrap();
    f.partial();
    fs::write(&path, "#\n".repeat(100_001)).unwrap();
    f.partial();
}

#[test]
fn parent_global_exclude_and_nested_rules_preserve_precedence_and_explicit_files() {
    let f = Fixture::new();
    fs::write(f.0.join(".gitignore"), "parent_git.py\n").unwrap();
    fs::write(
        f.0.join("system.config"),
        "[core]\nexcludesFile = .global.rules\n",
    )
    .unwrap();
    fs::write(f.0.join("scope/.global.rules"), "global.py\n").unwrap();
    fs::create_dir_all(f.0.join("scope/.git/info")).unwrap();
    fs::write(f.0.join("scope/.git/info/exclude"), "excluded.py\n").unwrap();
    fs::write(f.0.join(".ignore"), "parent.py\n").unwrap();
    fs::write(f.0.join("scope/.gitignore"), "nested.py\n").unwrap();
    fs::write(f.0.join("scope/sub/.gitignore"), "!nested.py\n").unwrap();
    for name in [
        "parent.py",
        "parent_git.py",
        "global.py",
        "excluded.py",
        "nested.py",
        "sub/nested.py",
    ] {
        fs::write(f.0.join("scope").join(name), "def hidden(): pass\n").unwrap();
    }
    let out = f.run(&["search", "def", ".", "--files-with-matches"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("keep.py") && text.contains("sub/nested.py"),
        "{text}"
    );
    assert!(
        !text.contains("parent.py")
            && !text.contains("parent_git.py")
            && !text.contains("global.py")
            && !text.contains("excluded.py"),
        "{text}"
    );
    let explicit = f.run(&["search", "hidden", "global.py"]);
    assert!(
        explicit.status.success()
            && String::from_utf8(explicit.stdout)
                .unwrap()
                .contains("global.py")
    );
}

#[test]
fn global_config_symlink_is_guarded_and_reports_partial() {
    let f = Fixture::new();
    fs::write(
        f.0.join("outside.config"),
        "excludesFile = .outside.rules\n",
    )
    .unwrap();
    fs::write(f.0.join("scope/.outside.rules"), "keep.py\n").unwrap();
    fs::remove_file(f.0.join("system.config")).unwrap();
    std::os::unix::fs::symlink("outside.config", f.0.join("system.config")).unwrap();
    f.partial();
}

#[test]
fn guarded_walker_matches_legacy_ignore_precedence_on_controlled_inputs() {
    use std::collections::BTreeSet;
    let f = Fixture::new();
    let scope = f.0.join("scope");
    fs::create_dir_all(scope.join("blocked")).unwrap();
    fs::create_dir_all(scope.join("sub/repo/.git")).unwrap();
    fs::write(scope.join(".gitignore"), "\u{feff}\u{feff}blocked/\nroot_ignored.py\nshared.py\nsub/override.py\n!.hidden_allowed.py\n").unwrap();
    fs::write(
        scope.join(".ignore"),
        "!root_ignored.py\nignore_everywhere.py\n",
    )
    .unwrap();
    fs::write(
        scope.join("sub/.gitignore"),
        "!override.py\n!ignore_everywhere.py\n",
    )
    .unwrap();
    for name in [
        "blocked/inside.py",
        "root_ignored.py",
        "shared.py",
        "sub/shared.py",
        "sub/override.py",
        "sub/ignore_everywhere.py",
        "sub/repo/shared.py",
        ".hidden.py",
        ".hidden_allowed.py",
    ] {
        fs::write(scope.join(name), "def keep(): pass\n").unwrap();
    }
    // The legacy reader is used only on this controlled, bounded fixture, never
    // on user/global/parent config. Production has no automatic ignore loading.
    let expected = ignore::WalkBuilder::new(&scope)
        .git_global(false)
        .parents(false)
        .require_git(false)
        .follow_links(false)
        .build()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| {
            entry
                .path()
                .strip_prefix(&scope)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<BTreeSet<_>>();
    let out = f.run(&["search", "keep", ".", "--files-with-matches"]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    let actual = text
        .lines()
        .filter_map(|line| line.strip_prefix("file="))
        .map(|path| serde_json::from_str::<String>(path).unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "{text}");
}
