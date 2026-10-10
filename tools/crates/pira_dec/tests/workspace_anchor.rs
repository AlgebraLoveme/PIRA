use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "pira-dec-anchor-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("main/child/deep")).unwrap();
        Self(root.canonicalize().unwrap())
    }

    fn main(&self) -> PathBuf {
        self.0.join("main")
    }

    fn child(&self) -> PathBuf {
        self.0.join("main/child/deep")
    }

    fn command(&self, cwd: &Path, anchor: Option<&Path>) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pira_dec"));
        cmd.current_dir(cwd)
            .env("PIRA_DEC_STORE_DIR", self.0.join("store"))
            .env_remove("PIRA_DEC_WORKSPACE_DIR")
            .env(
                "CODEX_THREAD_ID",
                if anchor.is_some() { "worker" } else { "main" },
            );
        if let Some(anchor) = anchor {
            cmd.env("PIRA_DEC_WORKSPACE_DIR", anchor);
        }
        cmd
    }

    fn add(&self, cwd: &Path, anchor: Option<&Path>, maker: &str) -> String {
        let output = self
            .command(cwd, anchor)
            .args([
                "add",
                "--context",
                "anchor visibility",
                "--choice",
                "share",
                "--choice",
                "isolate",
                "--decision",
                "1",
                "--maker",
                maker,
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout)
            .unwrap()
            .split(" | ")
            .next()
            .unwrap()
            .to_owned()
    }

    fn search(&self, cwd: &Path, anchor: Option<&Path>) -> Vec<Value> {
        rows(
            self.command(cwd, anchor)
                .args(["search", "visibility", "--json"])
                .output()
                .unwrap(),
        )
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn rows(output: Output) -> Vec<Value> {
    assert!(matches!(output.status.code(), Some(0 | 1)), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    json["matches"].as_array().unwrap().clone()
}

#[test]
fn non_git_descendants_share_existing_ids_bidirectionally_without_priority_or_duplicates() {
    let s = Sandbox::new();
    let main = s.add(&s.main(), None, "agent");
    assert_eq!(s.search(&s.child(), Some(&s.main()))[0]["id"], main);
    let worker = s.add(&s.child(), Some(&s.main()), "human");
    let expected = s.search(&s.main(), None);
    assert_eq!(s.search(&s.child(), Some(&s.main())), expected);
    assert_eq!(expected.len(), 2);
    assert!(expected.iter().any(|r| r["id"] == worker));
    let keys: Vec<_> = expected
        .iter()
        .map(|r| {
            (
                r["timestamp_ms"].as_u64().unwrap(),
                r["id"].as_str().unwrap(),
            )
        })
        .collect();
    let mut ordered = keys.clone();
    ordered.sort_by(|a, b| b.cmp(a));
    assert_eq!(keys, ordered);
    let limited = rows(
        s.command(&s.child(), Some(&s.main()))
            .args(["search", "visibility", "--json", "--limit", "1"])
            .output()
            .unwrap(),
    );
    assert_eq!(limited, expected[..1]);
    let namespaces: Vec<_> = fs::read_dir(s.0.join("store")).unwrap().collect();
    assert_eq!(namespaces.len(), 1);
    assert_eq!(
        fs::read_dir(namespaces[0].as_ref().unwrap().path().join("records"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn no_anchor_preserves_non_git_cwd_isolation() {
    let s = Sandbox::new();
    s.add(&s.main(), None, "agent");
    assert!(s.search(&s.child(), None).is_empty());
    s.add(&s.child(), None, "human");
    assert_eq!(s.search(&s.main(), None).len(), 1);
}

#[test]
fn anchor_does_not_cross_nested_git_unrelated_git_or_physical_ancestor_boundaries() {
    let s = Sandbox::new();
    s.add(&s.main(), None, "agent");
    let nested = s.main().join("nested");
    let unrelated_git = s.0.join("git");
    let unrelated_plain = s.0.join("other");
    let lexical_prefix = s.0.join("main-extra");
    for cwd in [&nested, &unrelated_git, &unrelated_plain, &lexical_prefix] {
        fs::create_dir_all(cwd).unwrap();
    }
    fs::write(nested.join(".git"), "gitdir: external-worktree").unwrap();
    fs::create_dir(unrelated_git.join(".git")).unwrap();
    for cwd in [&nested, &unrelated_git, &unrelated_plain, &lexical_prefix] {
        assert!(s.search(cwd, Some(&s.main())).is_empty(), "{cwd:?}");
        s.add(cwd, Some(&s.main()), "human");
        assert_eq!(s.search(&s.main(), None).len(), 1);
        assert_eq!(s.search(cwd, None).len(), 1);
    }
}

#[test]
fn anchor_preserves_git_root_and_explicit_store_semantics() {
    let s = Sandbox::new();
    fs::create_dir(s.main().join(".git")).unwrap();
    let main = s.add(&s.main(), None, "agent");
    assert_eq!(s.search(&s.child(), Some(&s.main()))[0]["id"], main);
    let mut cmd = s.command(&s.child(), Some(&s.main()));
    let output = cmd
        .args([
            "add",
            "--store-dir",
            "local-store",
            "--context",
            "visibility explicit",
            "--choice",
            "one",
            "--choice",
            "two",
            "--decision",
            "1",
            "--maker",
            "human",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(s.child().join("local-store").is_dir());
    assert!(!s.main().join("local-store").exists());
    assert_eq!(s.search(&s.main(), None).len(), 1);
    let alternate = rows(
        s.command(&s.main(), None)
            .args(["search", "visibility", "--json", "--store-dir"])
            .arg(s.child().join("local-store"))
            .output()
            .unwrap(),
    );
    assert_eq!(alternate.len(), 1);
    assert_ne!(alternate[0]["id"], main);
}

#[test]
fn invalid_anchors_fail_visibly_before_writes() {
    let s = Sandbox::new();
    let file = s.0.join("file");
    fs::write(&file, "not a directory").unwrap();
    for path in [PathBuf::from("relative"), s.0.join("missing"), file] {
        let output = s
            .command(&s.child(), Some(&path))
            .args(["list", "--json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("PIRA_DEC_WORKSPACE_DIR")
        );
    }
    assert!(!s.0.join("store").exists());
}

#[cfg(unix)]
#[test]
fn physical_anchor_resolves_aliases_but_does_not_follow_descendant_symlink_escapes() {
    use std::os::unix::fs::symlink;
    let s = Sandbox::new();
    let main = s.add(&s.main(), None, "agent");
    let alias = s.0.join("alias");
    symlink(s.main(), &alias).unwrap();
    assert_eq!(
        s.search(&alias.join("child/deep"), Some(&s.main()))[0]["id"],
        main
    );
    let invalid = s
        .command(&s.child(), Some(&alias))
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2), "{invalid:?}");
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("canonical directory")
    );
    let outside = s.0.join("outside");
    fs::create_dir(&outside).unwrap();
    let escape = s.main().join("escape");
    symlink(&outside, &escape).unwrap();
    assert!(s.search(&escape, Some(&s.main())).is_empty());
    s.add(&escape, Some(&s.main()), "human");
    assert_eq!(s.search(&s.main(), None).len(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn native_non_utf8_anchor_shares_without_lossy_identity_conversion() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let s = Sandbox::new();
    let native = s.0.join(OsString::from_vec(b"native-\xff".to_vec()));
    let child = native.join("child");
    fs::create_dir_all(&child).unwrap();
    let main = s.add(&native, None, "agent");
    assert_eq!(s.search(&child, Some(&native))[0]["id"], main);
}

#[cfg(unix)]
#[test]
fn invalid_native_anchor_fails_without_lossy_fallback() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let s = Sandbox::new();
    let native = s.0.join(OsString::from_vec(b"missing-\xff".to_vec()));
    let output = s
        .command(&s.child(), Some(&native))
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("PIRA_DEC_WORKSPACE_DIR")
    );
    assert!(!s.0.join("store").exists());
}
