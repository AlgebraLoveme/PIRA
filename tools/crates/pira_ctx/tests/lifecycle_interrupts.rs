#![cfg(unix)]
use std::process::Command;

fn probe(case: &str) {
    let result = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/lifecycle_interrupts_probe.py"
        ))
        .args([env!("CARGO_BIN_EXE_pira_ctx"), case])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{case}: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn watch_probe_signals_stop_owned_helpers_and_persist_idle_state() {
    probe("probe");
}
#[test]
fn watch_analyzer_signals_stop_owned_helpers_and_persist_idle_state() {
    probe("analyzer");
}
#[test]
fn watch_idle_wait_observes_signals() {
    probe("idle");
}
#[test]
fn exec_stdin_signals_exit_without_eof_or_analysis_launch() {
    probe("stdin");
}
#[test]
fn exec_preflight_signals_reap_helper_tree_without_analysis_launch() {
    probe("preflight");
}
#[test]
fn blocked_exact_stdout_and_stderr_recover_without_pipe_drain() {
    probe("replay");
}
#[test]
fn blocked_auto_and_exec_replay_recover_without_pipe_drain() {
    probe("short_replay");
}
#[test]
fn successful_completion_has_clean_diagnostics_and_preserves_bytes() {
    probe("normal");
}
