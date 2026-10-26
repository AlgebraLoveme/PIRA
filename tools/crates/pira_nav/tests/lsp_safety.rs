#![cfg(unix)]

use std::{
    ffi::CString,
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pira-nav-pipes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let fixture = Self(path.canonicalize().unwrap());
        fs::write(
            fixture.0.join("source.py"),
            format!("#{}\ndef target(): pass\n", "x".repeat(1024 * 1024)),
        )
        .unwrap();
        fixture
    }

    fn run(&self, args: &[&str]) -> (Output, Duration) {
        let start = Instant::now();
        let mut child = Command::new(env!("CARGO_BIN_EXE_pira_nav"))
            .args(args)
            .current_dir(&self.0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(6) {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("bounded Nav probe timed out: {args:?}: {output:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        (child.wait_with_output().unwrap(), start.elapsed())
    }

    fn lsp(&self, mode: &str) -> (Output, Duration) {
        let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/lsp_safety_server.py");
        self.run(&[
            "definition",
            "source.py:2:5",
            "--lsp",
            "/usr/bin/env",
            "--lsp-arg",
            "python3",
            "--lsp-arg",
            server.to_str().unwrap(),
            "--lsp-arg",
            mode,
        ])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn unterminated_header_rejected_without_waiting_for_eof() {
    let f = Fixture::new();
    let (output, elapsed) = f.lsp("header");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("headers exceed"),
        "{output:?}"
    );
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}

#[test]
fn notifications_and_large_document_write_make_simultaneous_progress() {
    let f = Fixture::new();
    let (output, elapsed) = f.lsp("duplex");
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("count=1"));
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}

#[test]
fn notification_overflow_fails_instead_of_blocking_reader() {
    let f = Fixture::new();
    let (output, _) = f.lsp("overflow");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("queue exceeds"),
        "{output:?}"
    );
}

#[test]
fn oversized_protocol_position_fails_before_rendering() {
    let f = Fixture::new();
    let (output, _) = f.lsp("integer");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("valid integer"));
}

#[test]
fn exited_leader_does_not_leave_descendant_pipes_blocking_cleanup() {
    let f = Fixture::new();
    let mut unrelated = Command::new("/usr/bin/env")
        .args(["python3", "-c", "import time; time.sleep(4)"])
        .spawn()
        .unwrap();
    let (output, elapsed) = f.lsp("descendant");
    let unrelated_alive = unrelated.try_wait().unwrap().is_none();
    if unrelated_alive {
        unrelated.kill().unwrap();
    }
    unrelated.wait().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert!(
        unrelated_alive,
        "cleanup must not terminate unrelated processes"
    );
}

#[test]
fn fifo_source_and_lsp_config_are_rejected_without_open_blocking() {
    let f = Fixture::new();
    let path = CString::new(f.0.join("pipe.py").to_str().unwrap()).unwrap();
    // SAFETY: path is a valid NUL-terminated temporary path owned by this fixture.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    for args in [
        vec!["show", "pipe.py:1-1"],
        vec![
            "outline",
            "source.py",
            "--lsp",
            "/usr/bin/env",
            "--lsp-init",
            "pipe.py",
        ],
        vec![
            "outline",
            "source.py",
            "--lsp",
            "/usr/bin/env",
            "--lsp-settings",
            "pipe.py",
        ],
    ] {
        let (output, elapsed) = f.run(&args);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("not a regular file"),
            "{output:?}"
        );
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    }
}

fn structural_probe(f: &Fixture, mode: &str, args: &[&str]) -> Output {
    let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/lsp_safety_server.py");
    let mut args = args.to_vec();
    args.extend([
        "--lsp",
        "/usr/bin/env",
        "--lsp-arg",
        "python3",
        "--lsp-arg",
        server.to_str().unwrap(),
        "--lsp-arg",
        mode,
    ]);
    f.run(&args).0
}

#[test]
fn changed_lsp_snapshot_never_renders_another_symbols_source() {
    let f = Fixture::new();
    fs::write(f.0.join("source.py"), "def alpha(): pass\n# outside\n").unwrap();
    let output = structural_probe(&f, "snapshot", &["symbols", "alpha", "source.py"]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("reason=changed-source complete=0"), "{text}");
    assert!(!text.contains("def other"), "{text}");
}

#[test]
fn half_open_lsp_endpoint_does_not_own_next_line() {
    let f = Fixture::new();
    fs::write(f.0.join("source.py"), "def alpha(): pass\n# outside\n").unwrap();
    let output = structural_probe(&f, "endpoint", &["show", "source.py:2"]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no named source item contains line 2")
    );
    let output = structural_probe(&f, "endpoint", &["show", "source.py:1"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("# outside"));
}

#[test]
fn lsp_uri_preserves_unix_backslash_and_encoded_characters() {
    let f = Fixture::new();
    let name = "literal\\é %name.py";
    fs::write(f.0.join(name), "def alpha(): pass\n# outside\n").unwrap();
    let output = structural_probe(&f, "uri", &["outline", name]);
    assert!(output.status.success(), "{output:?}");
    let uri = fs::read_to_string(f.0.join("uri.txt")).unwrap();
    assert!(uri.ends_with("literal%5C%C3%A9%20%25name.py"), "{uri}");
}
