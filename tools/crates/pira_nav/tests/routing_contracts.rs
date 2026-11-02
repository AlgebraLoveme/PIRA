use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const DIRTY: &str = "def native_name(): pass\nif (\n";

struct Fixture {
    base: PathBuf,
    project: PathBuf,
    bin: PathBuf,
    marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "nav-routing-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let project = base.join("project");
        let bin = base.join("bin");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir(&bin).unwrap();
        let f = Self {
            base: base.canonicalize().unwrap(),
            project: project.canonicalize().unwrap(),
            bin: bin.canonicalize().unwrap(),
            marker: project.canonicalize().unwrap().join("started"),
        };
        for name in ["pylsp", "lake"] {
            f.launcher(
                name,
                if cfg!(windows) {
                    "@echo off\r\necho started>\"%NAV_ROUTING_MARKER%\"\r\nexit /b 73\r\n"
                } else {
                    "#!/bin/sh\nprintf started > \"$NAV_ROUTING_MARKER\"\nexit 73\n"
                },
            );
        }
        // Verify the sentinel actually executes on this platform before using
        // its absence as evidence that Navigator did not launch a server.
        let control = Command::new(f.server("pylsp"))
            .env("NAV_ROUTING_MARKER", &f.marker)
            .output()
            .unwrap();
        assert_eq!(control.status.code(), Some(73), "{control:?}");
        assert!(f.marker.is_file());
        fs::remove_file(&f.marker).unwrap();
        f
    }

    fn server(&self, name: &str) -> PathBuf {
        self.bin.join(if cfg!(windows) {
            format!("{name}.cmd")
        } else {
            name.to_string()
        })
    }

    fn launcher(&self, name: &str, text: &str) {
        let path = self.server(name);
        fs::write(&path, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pira_nav"));
        command
            .current_dir(&self.project)
            .args(args)
            .env("PATH", &self.bin)
            .env("NAV_ROUTING_MARKER", &self.marker);
        #[cfg(windows)]
        command.env("PATHEXT", ".COM;.EXE;.BAT;.CMD");
        command.output().unwrap()
    }

    fn no_spawn(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            !self.marker.exists(),
            "implicit server launch for {args:?}: {output:?}"
        );
        output
    }

    fn protocol_server(&self) -> PathBuf {
        let python = Command::new(if cfg!(windows) { "python" } else { "python3" })
            .args([
                "-c",
                "import os,sys; print(os.path.realpath(sys.executable))",
            ])
            .output()
            .unwrap();
        assert!(python.status.success(), "{python:?}");
        let python = PathBuf::from(String::from_utf8(python.stdout).unwrap().trim());
        let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantic_server.py");
        fs::write(
            self.project.join(".nav-lsp.json"),
            json!({
                "startup_log": self.marker,
                "default": ["other.py"]
            })
            .to_string(),
        )
        .unwrap();
        let script = if cfg!(windows) {
            format!(
                "@echo off\r\n\"{}\" \"{}\"\r\nexit /b %errorlevel%\r\n",
                python.display(),
                peer.display()
            )
        } else {
            let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
            format!("#!/bin/sh\nexec {} {}\n", quote(&python), quote(&peer))
        };
        self.launcher("pylsp", &script);
        python
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn map_keeps_dirty_file_topology_without_implicit_server_or_requirement() {
    for (name, source) in [
        ("dirty.py", DIRTY),
        ("dirty.lean", "def broken := (\n"),
        ("dirty.yml", "steps: [\n"),
        ("dirty.ps1", "function broken {\n"),
        ("dirty.json", "{\"broken\":\n"),
    ] {
        let f = Fixture::new();
        fs::write(f.project.join(name), source).unwrap();
        let output = f.no_spawn(&["map", ".", "--limit", "1"]);
        assert!(output.status.success(), "{name}: {output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(&format!("file=\"{name}\"")), "{text}");
        assert!(
            text.contains("landmarks_unavailable=1") && text.contains("complete=0"),
            "{text}"
        );
        assert!(
            !text.contains("requires an LSP") && !text.contains("install a conventional"),
            "{text}"
        );
    }
}

#[test]
fn map_partial_landmarks_preserve_clean_inventory_and_row_budget() {
    let f = Fixture::new();
    fs::write(f.project.join("a.py"), "def native_name(): pass\n").unwrap();
    fs::write(f.project.join("b.py"), DIRTY).unwrap();
    for extra in [vec![], vec!["--native"]] {
        let mut args = vec!["map", ".", "--limit", "2"];
        args.extend(extra);
        let output = f.no_spawn(&args);
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("parsed=1 failed=1 complete=0"), "{text}");
        assert!(
            text.contains("file=\"a.py\" language=python symbols=\"native_name\""),
            "{text}"
        );
        assert!(
            text.contains("file=\"b.py\" language=python landmarks_unavailable=1"),
            "{text}"
        );
    }
    let output = f.no_spawn(&["map", ".", "--limit", "1"]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("shown=1") && text.contains("omitted=1"),
        "{text}"
    );
}

#[test]
fn dirty_structural_inventory_does_not_use_path_servers() {
    let f = Fixture::new();
    fs::write(f.project.join("dirty.py"), DIRTY).unwrap();
    for args in [
        vec!["outline", "dirty.py"],
        vec!["outline", "dirty.py", "--native"],
        vec!["symbols", "native_name", "dirty.py"],
        vec!["show", "dirty.py::native_name"],
        vec!["show", "dirty.py:1:6"],
        vec!["query", "--show", "dirty.py::native_name"],
    ] {
        let output = f.no_spawn(&args);
        assert!(!output.status.success(), "{args:?}: {output:?}");
        let diagnostic = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(diagnostic.contains("native parser found"), "{diagnostic}");
        assert!(!diagnostic.contains("requires an LSP"), "{diagnostic}");
    }
}

#[test]
fn exact_reads_search_and_clean_native_inventory_do_not_use_servers() {
    let f = Fixture::new();
    fs::write(f.project.join("dirty.py"), DIRTY).unwrap();
    fs::write(f.project.join("clean.py"), "def native_name(): pass\n").unwrap();
    for args in [
        vec!["show", "dirty.py"],
        vec!["show", "dirty.py:1-2"],
        vec!["show", "dirty.py:1:6", "--window", "1"],
        vec!["query", "--show", "dirty.py:1-2"],
        vec!["search", "native_name", "."],
        vec!["search", "native_name", ".", "--owners"],
        vec!["outline", "clean.py"],
        vec!["outline", "clean.py", "--native"],
    ] {
        let output = f.no_spawn(&args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("native_name"),
            "{output:?}"
        );
    }
}

#[test]
fn explicit_inventory_and_discovered_semantics_still_use_the_server() {
    let f = Fixture::new();
    fs::write(f.project.join("clean.py"), "def native_name(): pass\n").unwrap();
    fs::write(f.project.join("dirty.py"), DIRTY).unwrap();
    fs::write(f.project.join("other.py"), "answer = 42\n").unwrap();
    let python = f.protocol_server();
    let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantic_server.py");
    let output = f.run(&[
        "outline",
        "clean.py",
        "--lsp",
        python.to_str().unwrap(),
        "--lsp-arg",
        peer.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("server_only") && !text.contains("native_name"),
        "{text}"
    );
    assert!(f.marker.is_file());
    fs::remove_file(&f.marker).unwrap();

    let output = f.run(&["definition", "clean.py:1:6"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("file=\"other.py\""),
        "{output:?}"
    );
    assert!(f.marker.is_file());
    fs::remove_file(&f.marker).unwrap();

    let output = f.run(&["hover", "dirty.py::server_only"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Fake semantic information"),
        "{output:?}"
    );
    assert!(f.marker.is_file());
    let requests = fs::read_to_string(f.project.join(".requests")).unwrap();
    assert_eq!(requests, "clean.py:0:5\ndirty.py:0:4\n");
}
