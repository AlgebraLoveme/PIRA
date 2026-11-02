#![cfg(unix)]

use std::process::Command;

fn probe(case: &str) {
    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/runtime_repairs_probe.py"
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
fn closed_capture_and_watch_diagnostics_do_not_abort() {
    probe("diagnostics");
}
#[test]
fn escaped_owned_child_is_killed_without_signalling_its_new_group() {
    probe("escape");
}
#[test]
fn batch_failure_cancels_and_joins_peers_in_either_order() {
    probe("batch");
}
#[test]
fn failed_publication_preserves_private_recoverable_output_and_unknown_exit() {
    probe("recovery");
}
#[test]
fn spool_write_failure_terminates_owned_child_promptly() {
    probe("reader");
}
#[test]
fn live_control_is_independent_of_caller_tmpdir() {
    probe("tmpdir");
}
#[test]
fn legacy_private_spools_are_readable_across_tmpdirs_but_aliases_are_rejected() {
    probe("legacy");
}
#[test]
fn accepted_watch_stop_overrides_analyzer_pipe_failure() {
    probe("watch_stop");
}
#[test]
fn short_auto_and_exact_replay_leave_no_interrupted_snapshot() {
    probe("replay");
}

#[test]
fn escaped_watch_attempts_are_reaped_on_timeout() {
    probe("watch_escape");
}
#[test]
fn watch_stop_never_acknowledges_an_unapplied_terminal_state() {
    probe("stop_race");
}

#[test]
fn successful_batch_preserves_specification_order_and_nonzero_child_results() {
    probe("batch_success");
}
