use std::process::Command;

fn run(case: &str) {
    let output = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/repairs_probe.py"
        ))
        .args([env!("CARGO_BIN_EXE_pira_ctx"), case])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{case}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn search_admits_unequal_query_hits_before_context() {
    run("search");
}
#[test]
fn successful_check_discloses_index_ceiling_without_losing_raw() {
    run("check");
}
#[test]
fn prune_uses_authoritative_age_and_oldest_first_size_order() {
    run("prune");
}
#[test]
fn crafted_metadata_cannot_redirect_rebuilt_index() {
    run("index");
}
#[test]
fn ephemeral_exec_preserves_unicode_path_api_and_snapshot_bytes() {
    run("exec");
}
#[test]
fn unreliable_pending_attention_preserves_terminal_precedence_and_cache() {
    run("attention");
}
#[cfg(unix)]
#[test]
fn failed_history_inventory_never_publishes_hidden_retained_history() {
    run("history");
}
#[cfg(unix)]
#[test]
fn concurrent_watch_controls_cannot_merge_invalid_process_cadence() {
    run("controls");
}
#[cfg(unix)]
#[test]
fn owner_dead_capture_cannot_be_monitored_as_trustworthy_pending() {
    run("interrupted");
}

#[cfg(unix)]
#[test]
fn live_checkpoints_preserve_native_paths_active_leases_and_pruning() {
    run("checkpoints");
}
