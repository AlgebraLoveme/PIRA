#![allow(dead_code)] // Each integration suite uses a different subset of this support.

use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

pub fn command(root: &Path) -> Command {
    let home = root.join(".nav-test-home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_pira_nav"));
    // Missing optional configuration is intentional; do not consult the host home.
    command
        .current_dir(root)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .env("GIT_CONFIG_SYSTEM", home.join("gitconfig"));
    command
}

pub fn lsp(root: &Path, args: &[&str], config: Value) -> Output {
    fs::write(root.join(".nav-lsp.json"), config.to_string()).unwrap();
    let server = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantic_server.py");
    command(root)
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
