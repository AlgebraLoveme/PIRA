#[cfg(unix)]
#[test]
fn bounded_index_contention_retains_output_and_recovers_concurrent_publishers() {
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/index_concurrency_probe.py"
        ))
        .arg(env!("CARGO_BIN_EXE_pira_ctx"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
