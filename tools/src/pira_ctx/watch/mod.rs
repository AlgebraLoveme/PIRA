mod analyzer;
pub(crate) mod process;
mod render;
mod sample;
mod state;

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::cli::{Config, WatchAttention};
use crate::model::ListedEntry;
use crate::{storage, util};
use render::TerminalView;
use state::{
    AnalyzerSpec, AttemptStatus, AttentionPolicy, ControlState, JobStatus, MonitorStatus,
    SourceKind, WatchConfiguration, WatchState,
};

const SCHEMA: u32 = 1;
const CONTROL_POLL_MS: u64 = 100;
const MAX_REPORT_BYTES: usize = 12 * 1024;
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn run(config: &Config) -> Result<i32, String> {
    let store = storage::effective_store_dir(config.store_dir.as_ref())?;
    if let Some(target) = config.target.as_deref() {
        let id = state::resolve(&store, target)?;
        if config.watch_latest {
            return latest(&store, &id);
        }
        if config.watch_stop {
            return stop(&store, &id);
        }
        let analyzer_update = config.watch_clear_analyzer
            || config.watch_analyzer_file.is_some()
            || config.watch_analyzer_code.is_some();
        let configuration_update = configuration_update_requested(config);
        if analyzer_update || configuration_update {
            return update_controls(config, &store, &id, analyzer_update, configuration_update);
        }
        return own(config, &store, &id);
    }
    let id = create(config, &store)?;
    eprintln!("PIRA watch live | result={id}");
    own(config, &store, &id)
}

fn create(config: &Config, store: &Path) -> Result<String, String> {
    let now = state::now_ms();
    let id = format!(
        "watch-{now}-{}-{}",
        std::process::id(),
        ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let cwd = std::env::current_dir()
        .map_err(|error| error.to_string())?
        .canonicalize()
        .map_err(|error| format!("resolve watch working directory: {error}"))?;
    let (source_kind, source, capture_path) = if config.watch_current {
        (
            SourceKind::Capture,
            vec![storage::resolve_current_live_capture(store)?],
            None,
        )
    } else if let Some(capture) = config.watch_capture.as_ref() {
        let path = storage::resolve_result(store, capture)?;
        let stored = storage::read_result_path(&path)?;
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("resolve capture path: {error}"))?;
        let canonical_store = store.canonicalize().unwrap_or_else(|_| store.to_path_buf());
        if stored.is_running() && !canonical.starts_with(&canonical_store) {
            return Err("watch cannot persist a live capture outside its store; use that store with --store-dir".into());
        }
        let persisted_path = (!canonical.starts_with(&canonical_store)).then_some(canonical);
        (
            SourceKind::Capture,
            vec![stored.metadata.result_id],
            persisted_path,
        )
    } else {
        (SourceKind::Probe, config.cmd.clone(), None)
    };
    let analyzer = configured_analyzer(config, store, 1)?;
    validate_sampling_cost(
        source_kind,
        analyzer.is_some(),
        config.watch_sample_every_ms,
    )?;
    let watch = WatchState {
        schema: SCHEMA,
        id: id.clone(),
        workspace_hash: storage::current_workspace_hash()?,
        created_ms: now,
        updated_ms: now,
        deadline_ms: now.saturating_add(u128::from(
            config
                .watch_deadline_ms
                .ok_or("watch requires --deadline")?,
        )),
        source_kind,
        source,
        source_cwd: cwd.display().to_string(),
        capture_path,
        intent: config.intent.clone(),
        sample_every_ms: config.watch_sample_every_ms,
        attempt_timeout_ms: config.watch_attempt_timeout_ms,
        pending_exit: config.watch_pending_exit,
        attention_policy: match config.watch_attention {
            WatchAttention::Return => AttentionPolicy::Return,
            WatchAttention::Cache => AttentionPolicy::Cache,
        },
        configuration_revision: 0,
        inactive_after_ms: config.watch_inactive_after_ms,
        unchanged_after_ms: config.watch_unchanged_after_ms,
        no_progress_after_ms: config.watch_no_progress_after_ms,
        analyzer,
        monitor: MonitorStatus::Active,
        job: JobStatus::Unknown,
        attempt: AttemptStatus::Idle,
        attempts: 0,
        sample_ms: None,
        next_sample_ms: now,
        stdout_offset: 0,
        stderr_offset: 0,
        raw_stdout: Vec::new(),
        raw_stderr: Vec::new(),
        visible_stdout: String::new(),
        visible_stderr: String::new(),
        stdout_view: TerminalView::default(),
        stderr_view: TerminalView::default(),
        raw_hash: String::new(),
        visible_hash: String::new(),
        progress_hash: String::new(),
        progress: String::new(),
        analyzer_summary: String::new(),
        analyzer_error: None,
        last_activity_ms: Some(now),
        last_visible_change_ms: Some(now),
        last_progress_ms: analyzer_present_time(config, now),
        attention_reason: None,
        attention_sequence: 0,
        detail: "watch created".into(),
        rendered_reliable: true,
    };
    state::write(&state::state_path(store, &id), &watch)?;
    state::write(&state::control_path(store, &id), &ControlState::default())?;
    Ok(id)
}

fn analyzer_present_time(config: &Config, now: u128) -> Option<u128> {
    (config.watch_analyzer_file.is_some() || config.watch_analyzer_code.is_some()).then_some(now)
}

fn configured_analyzer(
    config: &Config,
    store: &Path,
    revision: u64,
) -> Result<Option<AnalyzerSpec>, String> {
    if let Some(path) = config.watch_analyzer_file.as_ref() {
        return analyzer::spec(store, &analyzer::load_code(path)?, revision).map(Some);
    }
    if let Some(code) = config.watch_analyzer_code.as_ref() {
        if code.len() > 1024 * 1024 {
            return Err("analyzer code exceeds 1 MiB".into());
        }
        return analyzer::spec(store, code, revision).map(Some);
    }
    Ok(None)
}

fn validate_sampling_cost(
    source_kind: SourceKind,
    has_analyzer: bool,
    sample_every_ms: u64,
) -> Result<(), String> {
    if (source_kind == SourceKind::Probe || has_analyzer) && sample_every_ms < 1_000 {
        return Err("process-backed watches require --sample-every of at least 1s".into());
    }
    Ok(())
}

fn own(config: &Config, store: &Path, id: &str) -> Result<i32, String> {
    let _owner = state::acquire_owner(store, id)?;
    let path = state::state_path(store, id);
    let mut watch: WatchState = state::read(&path, "watch state")?;
    validate(&watch, id)?;
    if terminal(&watch) {
        return report(&watch, exit_code(&watch));
    }
    if watch.monitor == MonitorStatus::Stopped {
        state::update_control(store, id, |control| control.stop_requested = false)?;
        watch.monitor = MonitorStatus::Active;
        watch.detail = "watch resumed".into();
    } else if watch.monitor == MonitorStatus::Paused {
        watch.monitor = MonitorStatus::Active;
        watch.detail = "watch resumed".into();
    }
    let review_deadline = config
        .watch_review_after_ms
        .map(|ms| state::now_ms().saturating_add(u128::from(ms)));
    loop {
        let now = state::now_ms();
        if terminal(&watch) {
            persist(&path, &mut watch)?;
            return report(&watch, exit_code(&watch));
        }
        apply_control(store, &mut watch)?;
        if watch.monitor == MonitorStatus::Stopped {
            persist(&path, &mut watch)?;
            return report(&watch, 23);
        }
        if now >= watch.deadline_ms {
            watch.monitor = MonitorStatus::Deadline;
            watch.detail = "overall deadline reached".into();
            persist(&path, &mut watch)?;
            return report(&watch, 21);
        }
        if review_deadline.is_some_and(|deadline| now >= deadline) {
            watch.monitor = MonitorStatus::Paused;
            watch.detail = "requested review interval reached".into();
            persist(&path, &mut watch)?;
            return report(&watch, 10);
        }
        if now < watch.next_sample_ms {
            let wake_at = watch
                .next_sample_ms
                .min(watch.deadline_ms)
                .min(review_deadline.unwrap_or(u128::MAX));
            sleep_controlled(store, &mut watch, wake_at)?;
            continue;
        }
        watch.attempt = AttemptStatus::Probing;
        watch.updated_ms = now;
        state::write(&path, &watch)?;
        let control_path = state::control_path(store, id);
        let attempt_limit_ms = remaining_ms(
            now,
            watch.deadline_ms,
            review_deadline,
            watch.attempt_timeout_ms,
        );
        let sample = sample::collect(&mut watch, store, attempt_limit_ms, || {
            state::read::<ControlState>(&control_path, "watch control")
                .is_ok_and(|c| c.stop_requested)
        });
        watch.attempt = AttemptStatus::Idle;
        watch.attempts = watch.attempts.saturating_add(1);
        let sample = match sample {
            Ok(sample) => sample,
            Err(error) if error == "watch stop requested" => {
                watch.monitor = MonitorStatus::Stopped;
                watch.detail = error;
                persist(&path, &mut watch)?;
                return report(&watch, 23);
            }
            Err(error) => {
                if state::now_ms() >= watch.deadline_ms {
                    watch.monitor = MonitorStatus::Deadline;
                    watch.detail = "overall deadline reached".into();
                    persist(&path, &mut watch)?;
                    return report(&watch, 21);
                }
                if review_deadline.is_some_and(|deadline| state::now_ms() >= deadline) {
                    watch.monitor = MonitorStatus::Paused;
                    watch.detail = "requested review interval reached".into();
                    persist(&path, &mut watch)?;
                    return report(&watch, 10);
                }
                watch.monitor = MonitorStatus::Failed;
                watch.detail = error;
                persist(&path, &mut watch)?;
                return report(&watch, 22);
            }
        };
        let run_analyzer = matches!(sample.job, JobStatus::Unknown | JobStatus::Pending);
        let analyzer_limit_ms = remaining_ms(
            state::now_ms(),
            watch.deadline_ms,
            review_deadline,
            watch.attempt_timeout_ms,
        );
        let analyzer_attention =
            match incorporate_sample(&mut watch, sample, store, run_analyzer, analyzer_limit_ms) {
                Ok(attention) => attention,
                Err(error) if error == "watch stop requested" => {
                    watch.monitor = MonitorStatus::Stopped;
                    watch.detail = error;
                    persist(&path, &mut watch)?;
                    return report(&watch, 23);
                }
                Err(error) => {
                    if state::now_ms() >= watch.deadline_ms {
                        watch.monitor = MonitorStatus::Deadline;
                        watch.detail = "overall deadline reached".into();
                        persist(&path, &mut watch)?;
                        return report(&watch, 21);
                    }
                    if review_deadline.is_some_and(|deadline| state::now_ms() >= deadline) {
                        watch.monitor = MonitorStatus::Paused;
                        watch.detail = "requested review interval reached".into();
                        persist(&path, &mut watch)?;
                        return report(&watch, 10);
                    }
                    watch.monitor = MonitorStatus::Failed;
                    watch.detail = error.clone();
                    watch.analyzer_error = Some(error);
                    persist(&path, &mut watch)?;
                    return report(&watch, 22);
                }
            };
        if watch.job == JobStatus::Succeeded {
            watch.monitor = MonitorStatus::Complete;
            persist(&path, &mut watch)?;
            return report(&watch, 0);
        }
        if watch.job == JobStatus::Failed {
            watch.monitor = MonitorStatus::Complete;
            persist(&path, &mut watch)?;
            return report(&watch, 20);
        }
        let attention = evaluate_attention(&mut watch, analyzer_attention);
        watch.next_sample_ms = state::now_ms().saturating_add(u128::from(watch.sample_every_ms));
        persist(&path, &mut watch)?;
        if attention && watch.attention_policy == AttentionPolicy::Return {
            watch.monitor = MonitorStatus::Paused;
            persist(&path, &mut watch)?;
            return report(&watch, 10);
        }
    }
}

fn incorporate_sample(
    watch: &mut WatchState,
    sample: sample::Sample,
    store: &Path,
    run_analyzer: bool,
    attempt_limit_ms: u64,
) -> Result<Option<String>, String> {
    let now = state::now_ms();
    watch.sample_ms = Some(now);
    watch.job = sample.job;
    watch.detail = sample.detail;
    if sample.activity {
        watch.last_activity_ms = Some(now)
    }
    if watch.source_kind == SourceKind::Capture {
        append_tail(&mut watch.raw_stdout, &sample.stdout);
        append_tail(&mut watch.raw_stderr, &sample.stderr);
    } else {
        watch.raw_stdout = sample.stdout.clone();
        watch.raw_stderr = sample.stderr.clone();
    }
    watch.raw_hash = sample::hash(&[&watch.raw_stdout, &watch.raw_stderr]);
    if watch.source_kind == SourceKind::Probe {
        watch.stdout_view = TerminalView::default();
        watch.stderr_view = TerminalView::default();
    }
    watch.stdout_view.feed(&sample.stdout);
    watch.stderr_view.feed(&sample.stderr);
    let stdout = watch.stdout_view.text();
    let stderr = watch.stderr_view.text();
    let visible_hash = sample::hash(&[stdout.as_bytes(), stderr.as_bytes()]);
    if visible_hash != watch.visible_hash {
        watch.last_visible_change_ms = Some(now)
    }
    watch.visible_hash = visible_hash;
    watch.visible_stdout = stdout;
    watch.visible_stderr = stderr;
    watch.rendered_reliable =
        sample.reliable && watch.stdout_view.reliable && watch.stderr_view.reliable;
    watch.analyzer_error = None;
    let mut attention = None;
    if run_analyzer && let Some(spec) = watch.analyzer.as_ref() {
        watch.attempt = AttemptStatus::Analyzing;
        let input = analyzer::AnalyzerInput {
            source: if watch.source_kind == SourceKind::Capture {
                "capture"
            } else {
                "probe"
            },
            job_state: job_label(watch.job),
            raw_stdout: &String::from_utf8_lossy(&watch.raw_stdout),
            raw_stderr: &String::from_utf8_lossy(&watch.raw_stderr),
            visible_stdout: &watch.visible_stdout,
            visible_stderr: &watch.visible_stderr,
            attempts: watch.attempts,
        };
        let control_path = state::control_path(store, &watch.id);
        match analyzer::run(
            spec,
            &input,
            attempt_limit_ms,
            store,
            Path::new(&watch.source_cwd),
            || {
                state::read::<ControlState>(&control_path, "watch control")
                    .is_ok_and(|control| control.stop_requested)
            },
        ) {
            Ok(output) => {
                let canonical = if output.progress.is_empty() {
                    output.summary.clone()
                } else {
                    output.progress.clone()
                };
                let hash = hash_text(&canonical);
                if hash != watch.progress_hash {
                    watch.last_progress_ms = Some(now);
                }
                watch.progress_hash = hash;
                watch.progress = canonical;
                watch.analyzer_summary = output.summary;
                if output.attention {
                    attention = Some("analyzer requested attention".to_string());
                }
            }
            Err(error) => {
                watch.attempt = AttemptStatus::Idle;
                return Err(error);
            }
        }
        watch.attempt = AttemptStatus::Idle;
    }
    watch.updated_ms = now;
    Ok(attention)
}

fn append_tail(tail: &mut Vec<u8>, growth: &[u8]) {
    const MAX_RAW_TAIL: usize = 16 * 1024;
    tail.extend_from_slice(growth);
    if tail.len() > MAX_RAW_TAIL {
        tail.drain(..tail.len() - MAX_RAW_TAIL);
    }
}

fn remaining_ms(now: u128, deadline: u128, review: Option<u128>, configured: u64) -> u64 {
    let boundary = deadline.min(review.unwrap_or(u128::MAX));
    let remaining = boundary.saturating_sub(now).min(u128::from(u64::MAX)) as u64;
    configured.min(remaining.max(1))
}

fn evaluate_attention(watch: &mut WatchState, analyzer_reason: Option<String>) -> bool {
    let now = state::now_ms();
    let reason = analyzer_reason.or_else(|| {
        if watch.inactive_after_ms.is_some_and(|limit| {
            now.saturating_sub(watch.last_activity_ms.unwrap_or(watch.created_ms))
                >= u128::from(limit)
        }) {
            Some("no raw activity observed".to_string())
        } else if watch.unchanged_after_ms.is_some_and(|limit| {
            now.saturating_sub(watch.last_visible_change_ms.unwrap_or(watch.created_ms))
                >= u128::from(limit)
        }) {
            Some("visible output unchanged".to_string())
        } else if watch.no_progress_after_ms.is_some_and(|limit| {
            now.saturating_sub(watch.last_progress_ms.unwrap_or(watch.created_ms))
                >= u128::from(limit)
        }) {
            Some("analyzer progress unchanged".to_string())
        } else {
            None
        }
    });
    update_attention(watch, reason)
}

fn update_attention(watch: &mut WatchState, reason: Option<String>) -> bool {
    if watch.attention_reason == reason {
        return false;
    }
    let raised = reason.is_some();
    watch.attention_reason = reason;
    if raised {
        watch.attention_sequence = watch.attention_sequence.saturating_add(1);
    }
    raised
}

fn apply_control(store: &Path, watch: &mut WatchState) -> Result<(), String> {
    let path = state::control_path(store, &watch.id);
    let control: ControlState = state::read(&path, "watch control")?;
    if control.stop_requested {
        watch.monitor = MonitorStatus::Stopped;
        watch.detail = "stop requested".into()
    }
    if control.clear_analyzer {
        watch.analyzer = None;
        watch.progress.clear();
        watch.progress_hash.clear();
        watch.analyzer_summary.clear();
        watch.last_progress_ms = None;
    }
    if let Some(spec) = control.analyzer
        && watch
            .analyzer
            .as_ref()
            .is_none_or(|old| spec.revision > old.revision)
    {
        watch.analyzer = Some(spec);
        watch.progress.clear();
        watch.progress_hash.clear();
        watch.analyzer_summary.clear();
        watch.last_progress_ms = Some(state::now_ms());
        watch.analyzer_error = None;
    }
    if let Some(configuration) = control.configuration
        && configuration.revision > watch.configuration_revision
    {
        watch.configuration_revision = configuration.revision;
        watch.sample_every_ms = configuration.sample_every_ms;
        watch.inactive_after_ms = configuration.inactive_after_ms;
        watch.unchanged_after_ms = configuration.unchanged_after_ms;
        watch.no_progress_after_ms = configuration.no_progress_after_ms;
        watch.attention_policy = configuration.attention_policy;
        watch.next_sample_ms = watch
            .next_sample_ms
            .min(state::now_ms().saturating_add(u128::from(watch.sample_every_ms)));
        watch.detail = format!("configuration revision {} applied", configuration.revision);
    }
    Ok(())
}

fn sleep_controlled(store: &Path, watch: &mut WatchState, until: u128) -> Result<(), String> {
    while state::now_ms() < until {
        thread::sleep(Duration::from_millis(
            CONTROL_POLL_MS.min(until.saturating_sub(state::now_ms()) as u64),
        ));
        apply_control(store, watch)?;
        if watch.monitor == MonitorStatus::Stopped {
            return Ok(());
        }
        if watch.next_sample_ms < until {
            return Ok(());
        }
    }
    Ok(())
}
fn persist(path: &Path, watch: &mut WatchState) -> Result<(), String> {
    watch.updated_ms = state::now_ms();
    state::write(path, watch)
}
fn terminal(w: &WatchState) -> bool {
    matches!(
        w.monitor,
        MonitorStatus::Complete | MonitorStatus::Deadline | MonitorStatus::Failed
    )
}
fn exit_code(w: &WatchState) -> i32 {
    match w.monitor {
        MonitorStatus::Complete if w.job == JobStatus::Succeeded => 0,
        MonitorStatus::Complete => 20,
        MonitorStatus::Deadline => 21,
        MonitorStatus::Failed => 22,
        MonitorStatus::Stopped => 23,
        _ => 10,
    }
}
fn job_label(j: JobStatus) -> &'static str {
    match j {
        JobStatus::Unknown => "unknown",
        JobStatus::Pending => "pending",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed => "failed",
    }
}
fn hash_text(value: &str) -> String {
    let mut h = Sha256::new();
    h.update(value.as_bytes());
    util::hex(&h.finalize())
}

fn latest(store: &Path, id: &str) -> Result<i32, String> {
    let watch: WatchState = state::read(&state::state_path(store, id), "watch state")?;
    validate(&watch, id)?;
    report(&watch, 0)
}
fn stop(store: &Path, id: &str) -> Result<i32, String> {
    let path = state::state_path(store, id);
    let watch: WatchState = state::read(&path, "watch state")?;
    validate(&watch, id)?;
    if terminal(&watch) || watch.monitor == MonitorStatus::Stopped {
        util::stdout_line(&format!(
            "watch {id} is already {:?}; state unchanged",
            watch.monitor
        ))?;
        return Ok(0);
    }
    state::update_control(store, id, |control| control.stop_requested = true)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let watch: WatchState = state::read(&state::state_path(store, id), "watch state")?;
        if watch.monitor == MonitorStatus::Stopped {
            util::stdout_line(&format!("watch {id} stopped; monitored job is unchanged"))?;
            return Ok(0);
        }
        if !state::owner_is_alive(store, id)
            && let Ok(_owner) = state::acquire_owner(store, id)
        {
            let mut latest: WatchState = state::read(&path, "watch state")?;
            if !terminal(&latest) {
                latest.monitor = MonitorStatus::Stopped;
                latest.attempt = AttemptStatus::Idle;
                latest.detail = "stop applied while watch had no owner".into();
                persist(&path, &mut latest)?;
            }
            util::stdout_line(&format!("watch {id} stopped; monitored job is unchanged"))?;
            return Ok(0);
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "stop queued for {id}, but the active owner did not acknowledge within 3s"
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}
fn update_controls(
    config: &Config,
    store: &Path,
    id: &str,
    analyzer_update: bool,
    mut configuration_update: bool,
) -> Result<i32, String> {
    let code = if let Some(path) = config.watch_analyzer_file.as_ref() {
        Some(analyzer::load_code(path)?)
    } else {
        config.watch_analyzer_code.clone()
    };
    let code_hash = code
        .as_deref()
        .map(|value| analyzer::store_code(store, value))
        .transpose()?;
    let state: WatchState = state::read(&state::state_path(store, id), "watch state")?;
    let existing_control: ControlState =
        state::read(&state::control_path(store, id), "watch control")?;
    let effective_analyzer = if analyzer_update {
        !config.watch_clear_analyzer
    } else if existing_control.clear_analyzer {
        existing_control.analyzer.is_some()
    } else {
        existing_control.analyzer.is_some() || state.analyzer.is_some()
    };
    let requested_no_progress = if config.watch_clear_analyzer {
        if config.watch_no_progress_after_set {
            config.watch_no_progress_after_ms
        } else {
            None
        }
    } else if config.watch_no_progress_after_set {
        config.watch_no_progress_after_ms
    } else {
        existing_control
            .configuration
            .as_ref()
            .map_or(state.no_progress_after_ms, |value| {
                value.no_progress_after_ms
            })
    };
    if requested_no_progress.is_some() && !effective_analyzer {
        return Err("--no-progress-after requires an effective analyzer".into());
    }
    let requested_sample_every = if config.watch_sample_every_set {
        config.watch_sample_every_ms
    } else {
        existing_control
            .configuration
            .as_ref()
            .map_or(state.sample_every_ms, |value| value.sample_every_ms)
    };
    validate_sampling_cost(
        state.source_kind,
        effective_analyzer,
        requested_sample_every,
    )?;
    let mut analyzer_revision = None;
    let mut configuration_revision = None;
    state::update_control(store, id, |control| {
        if analyzer_update {
            let revision = control
                .analyzer_revision
                .max(state.analyzer.as_ref().map_or(0, |value| value.revision))
                .saturating_add(1);
            analyzer_revision = Some(revision);
            control.analyzer_revision = revision;
            control.clear_analyzer = config.watch_clear_analyzer;
            control.analyzer = code_hash.clone().map(|code_hash| AnalyzerSpec {
                revision,
                code_hash,
                code: String::new(),
            });
            if config.watch_clear_analyzer && !config.watch_no_progress_after_set {
                configuration_update = true;
            }
        }
        if !configuration_update {
            return;
        }
        let current = control.configuration.as_ref();
        let revision = control
            .configuration_revision
            .max(state.configuration_revision)
            .saturating_add(1);
        configuration_revision = Some(revision);
        control.configuration_revision = revision;
        control.configuration = Some(WatchConfiguration {
            revision,
            sample_every_ms: if config.watch_sample_every_set {
                config.watch_sample_every_ms
            } else {
                current.map_or(state.sample_every_ms, |value| value.sample_every_ms)
            },
            inactive_after_ms: if config.watch_inactive_after_set {
                config.watch_inactive_after_ms
            } else {
                current.map_or(state.inactive_after_ms, |value| value.inactive_after_ms)
            },
            unchanged_after_ms: if config.watch_unchanged_after_set {
                config.watch_unchanged_after_ms
            } else {
                current.map_or(state.unchanged_after_ms, |value| value.unchanged_after_ms)
            },
            no_progress_after_ms: if config.watch_clear_analyzer {
                None
            } else if config.watch_no_progress_after_set {
                config.watch_no_progress_after_ms
            } else {
                current.map_or(state.no_progress_after_ms, |value| {
                    value.no_progress_after_ms
                })
            },
            attention_policy: if config.watch_attention_set {
                match config.watch_attention {
                    WatchAttention::Return => AttentionPolicy::Return,
                    WatchAttention::Cache => AttentionPolicy::Cache,
                }
            } else {
                current.map_or(state.attention_policy, |value| value.attention_policy)
            },
        });
    })?;
    let analyzer =
        analyzer_revision.map_or_else(String::new, |revision| format!(" analyzer={revision}"));
    let configuration = configuration_revision
        .map_or_else(String::new, |revision| format!(" configuration={revision}"));
    util::stdout_line(&format!(
        "watch {id} update queued:{analyzer}{configuration}"
    ))?;
    Ok(0)
}

fn configuration_update_requested(config: &Config) -> bool {
    config.watch_sample_every_set
        || config.watch_inactive_after_set
        || config.watch_unchanged_after_set
        || config.watch_no_progress_after_set
        || config.watch_attention_set
}

fn report(w: &WatchState, code: i32) -> Result<i32, String> {
    let mut out = util::BoundedStdout::new(MAX_REPORT_BYTES);
    out.line("PIRA watch")?;
    out.line(&format!("Result: {}", w.id))?;
    let final_probe = terminal(w) && w.source_kind == SourceKind::Probe;
    if final_probe {
        let outcome = w.detail.strip_prefix("probe ").unwrap_or(&w.detail);
        out.line(&format!(
            "Monitor: {:?} | Job: {:?} | Probe: {outcome}",
            w.monitor, w.job
        ))?;
    } else if terminal(w) {
        out.line(&format!("Monitor: {:?} | Job: {:?}", w.monitor, w.job))?;
    } else {
        out.line(&format!(
            "Monitor: {:?} | Job: {:?} | Attempt: {:?}",
            w.monitor, w.job, w.attempt
        ))?;
    }
    out.line(&format!(
        "Samples: {} | Latest age: {}ms",
        w.attempts,
        w.sample_ms.map_or(0, |t| state::now_ms().saturating_sub(t))
    ))?;
    out.line(&format!("Last raw activity: {}", age(w.last_activity_ms)))?;
    out.line(&format!(
        "Last visible change: {} | render reliable: {}",
        age(w.last_visible_change_ms),
        w.rendered_reliable
    ))?;
    if let Some(analyzer) = w.analyzer.as_ref() {
        out.line(&format!(
            "Last semantic progress: {} | analyzer revision: {}",
            age(w.last_progress_ms),
            analyzer.revision
        ))?
    }
    if let Some(reason) = w.attention_reason.as_ref() {
        out.line(&format!("Attention: {reason}"))?
    }
    if let Some(error) = w.analyzer_error.as_ref() {
        out.line(&format!(
            "Analyzer error: {}",
            util::single_line_clip(error, 1000)
        ))?
    }
    if !w.progress.is_empty() {
        out.line("Latest analyzed progress:")?;
        out.line(&w.progress)?
    }
    if !w.analyzer_summary.is_empty() && w.analyzer_summary != w.progress {
        out.line("Latest analyzer summary:")?;
        out.line(&w.analyzer_summary)?
    }
    if !w.visible_stdout.is_empty() {
        out.line("Latest visible stdout:")?;
        out.line(&w.visible_stdout)?
    }
    if !w.visible_stderr.is_empty() {
        out.line("Latest visible stderr:")?;
        out.line(&w.visible_stderr)?
    }
    if !final_probe {
        out.line(&format!("Detail: {}", w.detail))?;
    }
    Ok(code)
}
fn age(value: Option<u128>) -> String {
    value.map_or_else(
        || "unavailable".into(),
        |v| format!("{}ms ago", state::now_ms().saturating_sub(v)),
    )
}

fn validate(w: &WatchState, id: &str) -> Result<(), String> {
    if w.schema != SCHEMA || w.id != id {
        return Err("watch state identity/schema mismatch".into());
    }
    state::validate_id(id)
}

pub fn list_entries(store: &Path, workspace: Option<&str>) -> Result<Vec<ListedEntry>, String> {
    let mut out = Vec::new();
    let dir = state::state_dir(store);
    if !dir.is_dir() {
        return Ok(out);
    }
    for item in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        if !item.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let path = item.path();
        let Ok(w) = state::read::<WatchState>(&path, "watch state") else {
            continue;
        };
        if workspace.is_some_and(|x| x != w.workspace_hash) {
            continue;
        }
        let listed_state = list_state(store, &w).to_string();
        let running = w.monitor == MonitorStatus::Active && state::owner_is_alive(store, &w.id);
        out.push(ListedEntry {
            id: w.id.clone(),
            filename: path.file_name().unwrap().to_string_lossy().into(),
            timestamp: storage::format_utc_timestamp(w.created_ms / 1000),
            start_ms: w.created_ms,
            exit: exit_code(&w),
            bytes: fs::metadata(&path).map_or(0, |m| m.len()),
            lines: 0,
            command: format!(
                "watch {:?}: {}",
                w.source_kind,
                util::redacted_argv_display(&w.source)
            ),
            path,
            workspace_hash: w.workspace_hash,
            kind: "watch".into(),
            state: listed_state,
            running,
        });
    }
    Ok(out)
}

fn list_state<'a>(store: &Path, watch: &'a WatchState) -> &'a str {
    match watch.monitor {
        MonitorStatus::Active if state::owner_is_alive(store, &watch.id) => "active",
        MonitorStatus::Active => "interrupted",
        MonitorStatus::Paused => "paused",
        MonitorStatus::Stopped => "stopped",
        MonitorStatus::Deadline => "deadline",
        MonitorStatus::Failed => "monitor-failed",
        MonitorStatus::Complete if watch.job == JobStatus::Succeeded => "succeeded",
        MonitorStatus::Complete => "job-failed",
    }
}
pub fn forget(store: &Path, target: &str) -> Result<Option<std::path::PathBuf>, String> {
    let id = match state::resolve(store, target) {
        Ok(id) => id,
        Err(_)
            if !target.starts_with("watch-")
                && !target.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if state::owner_is_alive(store, &id) {
        return Err("stop active watch before forgetting it".into());
    }
    let _owner = state::acquire_owner(store, &id)?;
    let path = state::state_path(store, &id);
    let _: WatchState = state::read(&path, "watch state")?;
    fs::remove_file(&path).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(state::control_path(store, &id));
    Ok(Some(path))
}
