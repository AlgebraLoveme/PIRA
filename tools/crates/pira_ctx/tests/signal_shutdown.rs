#![cfg(unix)]

use std::process::Command;

fn probe(case: &str) {
    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/signal_shutdown_probe.py"
        ))
        .args([env!("CARGO_BIN_EXE_pira_ctx"), case])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{case}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn supported_cancel_retains_partial_output_and_cleans_descendants() {
    probe("cancel");
}
#[test]
fn sigint_finalizes_capture() {
    probe("int");
}
#[test]
fn sigterm_finalizes_capture() {
    probe("term");
}
#[test]
fn foreground_group_sigint_finalizes_capture() {
    probe("group");
}
#[test]
fn terminal_ctrl_c_finalizes_capture() {
    probe("tty");
}
#[test]
fn completed_child_status_survives_descendant_cleanup() {
    probe("completion");
}
#[test]
fn signals_retain_short_auto_exact_and_check_output() {
    probe("modes");
}
#[test]
fn signal_cancels_all_batch_workers_without_starting_pending_commands() {
    probe("batch");
}
#[test]
fn signal_with_publication_failure_retains_recoverable_checkpoint() {
    probe("failure");
}
#[test]
fn missing_short_handle_has_scoped_actionable_diagnostic() {
    probe("handle");
}
