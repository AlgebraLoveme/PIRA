use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

pub fn lsp(root: &Path, args: &[&str], config: Value) -> Output {
    fs::write(root.join(".nav-lsp.json"), config.to_string()).unwrap();
    let server = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantic_server.py");
    Command::new(env!("CARGO_BIN_EXE_pira_nav"))
        .current_dir(root)
        .args(args)
        .args(["--lsp", "/usr/bin/env", "--lsp-arg", "python3", "--lsp-arg"])
        .arg(server)
        .output()
        .unwrap()
}

pub fn success(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}
