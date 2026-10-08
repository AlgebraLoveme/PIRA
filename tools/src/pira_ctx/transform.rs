use serde::Deserialize;
use std::collections::{BTreeMap, HashSet, VecDeque};

use crate::cli::TransformOptions;
use crate::model::StreamKind;
use crate::storage::StoredResult;
use crate::util;

const MAX_MATERIALIZED_ROWS: usize = 1_000_000;
const MAX_MATERIALIZED_BYTES: usize = 128 * 1024 * 1024;
const MAX_UNIQUE_VALUES: usize = 100_000;
const MAX_RETURN_BYTES: usize = 64 * 1024 + 1;
const MAX_PLAN_BYTES: u64 = 1024 * 1024;
const MAX_PLAN_STEPS: usize = 64;
const MAX_CONTEXT_ROWS: usize = 10_000;

#[derive(Debug, Deserialize)]
struct Plan {
    steps: Vec<Step>,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Step {
    Match {
        regex: String,
    },
    Context {
        regex: String,
        #[serde(default)]
        before: usize,
        #[serde(default)]
        after: usize,
    },
    Exclude {
        regex: String,
    },
    Head {
        n: usize,
    },
    Tail {
        n: usize,
    },
    Unique,
    Count,
    GroupCount,
    Sort {
        #[serde(default)]
        numeric: bool,
    },
    Top {
        n: usize,
    },
    Sum,
    Min,
    Max,
    Mean,
    JsonField {
        field: String,
    },
    JsonEq {
        field: String,
        value: serde_json::Value,
    },
    Column {
        index: usize,
        #[serde(default = "default_delim")]
        delimiter: String,
    },
    Stream {
        stream: String,
    },
    Diagnostic,
}
fn default_delim() -> String {
    "\t".into()
}

#[derive(Clone)]
struct Row {
    text: String,
    stream: StreamKind,
}

pub fn run(store: &StoredResult, options: &TransformOptions) -> Result<Vec<String>, String> {
    if store.metadata.timeline_truncated {
        return Err("transform requires a complete line index; use raw --stdout/--stderr for retained stream bytes".into());
    }
    if options.plan.is_none() {
        return stream_direct(store, options);
    }
    let path = options.plan.as_ref().unwrap();
    let bytes = util::read_file_limited(path, MAX_PLAN_BYTES, "transform plan")?;
    let plan: Plan =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid transform plan: {e}"))?;
    validate_steps(&plan.steps)?;
    if options.count && !plan.steps.is_empty() {
        return Err("count is terminal; cannot follow --count with plan steps".into());
    }
    let mut rows = load_rows(store)?;
    for pattern in &options.matches {
        rows = filter(rows, pattern, true)?;
    }
    for pattern in &options.excludes {
        rows = filter(rows, pattern, false)?;
    }
    if options.unique {
        rows = unique(rows);
    }
    if let Some(n) = options.head {
        rows.truncate(n)
    }
    if let Some(n) = options.tail {
        let drain = rows.len().saturating_sub(n);
        rows.drain(..drain);
    }
    if options.count {
        return Ok(vec![rows.len().to_string()]);
    }
    apply(rows, &plan.steps)
}

fn check_materialization_budget(used: usize, next: u64) -> Result<(), String> {
    if next > MAX_MATERIALIZED_BYTES.saturating_sub(used) as u64 {
        return Err("transform plan input exceeds 128 MiB; use direct transform flags or a narrower capture".into());
    }
    Ok(())
}

fn load_rows(store: &StoredResult) -> Result<Vec<Row>, String> {
    let mut reader = store.reader()?;
    if store.metadata.line_timeline.len() > MAX_MATERIALIZED_ROWS {
        return Err(format!(
            "transform plan needs to materialize too many rows (limit {MAX_MATERIALIZED_ROWS}); use direct transform flags or a narrower capture"
        ));
    }
    let mut rows = Vec::with_capacity(store.metadata.line_timeline.len());
    let mut bytes = 0_usize;
    for line in &store.metadata.line_timeline {
        check_materialization_budget(bytes, line.length)?;
        let mut line_bytes = Vec::new();
        reader.copy_line(line, &mut line_bytes)?;
        bytes = bytes
            .checked_add(line_bytes.len())
            .ok_or("transform input size overflow")?;
        if bytes > MAX_MATERIALIZED_BYTES {
            return Err(format!(
                "transform plan input exceeds {} MiB; use direct transform flags or a narrower capture",
                MAX_MATERIALIZED_BYTES / 1024 / 1024
            ));
        }
        rows.push(Row {
            text: String::from_utf8_lossy(&line_bytes)
                .trim_end_matches(['\r', '\n'])
                .to_string(),
            stream: line.stream,
        })
    }
    Ok(rows)
}

fn stream_direct(store: &StoredResult, options: &TransformOptions) -> Result<Vec<String>, String> {
    let matches = compile_patterns(&options.matches)?;
    let excludes = compile_patterns(&options.excludes)?;
    if options.tail.is_some_and(|n| n > MAX_MATERIALIZED_ROWS) {
        return Err(format!("--tail is limited to {MAX_MATERIALIZED_ROWS} rows"));
    }
    let mut reader = store.reader()?;
    let mut seen = HashSet::new();
    let mut seen_bytes = 0;
    let mut selected = 0_usize;
    let mut output = Vec::new();
    let mut tail = VecDeque::new();
    let mut tail_bytes = 0_usize;
    let mut output_bytes = 0_usize;
    for line in &store.metadata.line_timeline {
        if options.head.is_some_and(|limit| selected >= limit) {
            break;
        }
        if line.length > MAX_MATERIALIZED_BYTES as u64 {
            return Err("transform line exceeds the 128 MiB safety limit".into());
        }
        let mut bytes = Vec::new();
        reader.copy_line(line, &mut bytes)?;
        let text = String::from_utf8_lossy(&bytes)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        if !matches.iter().all(|re| re.is_match(&text))
            || excludes.iter().any(|re| re.is_match(&text))
        {
            continue;
        }
        if options.unique && !retain_unique(&mut seen, &mut seen_bytes, &text)? {
            continue;
        }
        selected += 1;
        if options.count {
            continue;
        }
        if let Some(limit) = options.tail {
            tail_bytes = tail_bytes.saturating_add(text.len() + 1);
            tail.push_back(text);
            if tail.len() > limit
                && let Some(removed) = tail.pop_front()
            {
                tail_bytes = tail_bytes.saturating_sub(removed.len() + 1);
            }
            if tail_bytes > MAX_MATERIALIZED_BYTES {
                return Err("--tail exceeds the 128 MiB materialization limit".into());
            }
            continue;
        }
        output_bytes = output_bytes.saturating_add(text.len() + 1);
        output.push(text);
        if output_bytes > MAX_RETURN_BYTES {
            break;
        }
    }
    if options.count {
        if let Some(limit) = options.tail {
            selected = selected.min(limit);
        }
        return Ok(vec![selected.to_string()]);
    }
    if options.tail.is_some() {
        return Ok(tail.into());
    }
    Ok(output)
}

fn retain_unique(
    seen: &mut HashSet<String>,
    bytes: &mut usize,
    text: &str,
) -> Result<bool, String> {
    if seen.contains(text) {
        return Ok(false);
    }
    if seen.len() >= MAX_UNIQUE_VALUES {
        return Err(format!(
            "--unique exceeded {MAX_UNIQUE_VALUES} distinct values; narrow the input first"
        ));
    }
    if text.len() > MAX_MATERIALIZED_BYTES.saturating_sub(*bytes) {
        return Err("--unique exceeds the 128 MiB retention limit; narrow the input first".into());
    }
    *bytes += text.len();
    seen.insert(text.to_owned());
    Ok(true)
}

fn compile_patterns(patterns: &[String]) -> Result<Vec<regex::Regex>, String> {
    patterns
        .iter()
        .map(|pattern| regex::Regex::new(pattern).map_err(|e| format!("invalid regex: {e}")))
        .collect()
}
fn filter(rows: Vec<Row>, pattern: &str, keep: bool) -> Result<Vec<Row>, String> {
    let re = regex::Regex::new(pattern).map_err(|e| format!("invalid regex: {e}"))?;
    Ok(rows
        .into_iter()
        .filter(|r| re.is_match(&r.text) == keep)
        .collect())
}
fn unique(rows: Vec<Row>) -> Vec<Row> {
    let mut seen = HashSet::new();
    rows.into_iter()
        .filter(|r| seen.insert(r.text.clone()))
        .collect()
}
fn validate_steps(steps: &[Step]) -> Result<(), String> {
    if steps.len() > MAX_PLAN_STEPS {
        return Err(format!(
            "transform plan is limited to {MAX_PLAN_STEPS} steps"
        ));
    }
    for (index, step) in steps.iter().enumerate().take(steps.len().saturating_sub(1)) {
        if matches!(
            step,
            Step::Count | Step::GroupCount | Step::Sum | Step::Min | Step::Max | Step::Mean
        ) {
            return Err(format!(
                "transform step {} is a terminal reduction; trailing steps are not allowed",
                index + 1
            ));
        }
    }
    Ok(())
}

fn apply(mut rows: Vec<Row>, steps: &[Step]) -> Result<Vec<String>, String> {
    validate_steps(steps)?;
    let diagnostic_re =
        regex::Regex::new("(?i)(error|failed|failure|panic|exception|warning)").unwrap();
    for step in steps {
        match step {
            Step::Match { regex } => rows = filter(rows, regex, true)?,
            Step::Context {
                regex,
                before,
                after,
            } => {
                if *before > MAX_CONTEXT_ROWS || *after > MAX_CONTEXT_ROWS {
                    return Err(format!(
                        "transform context before/after are limited to {MAX_CONTEXT_ROWS} rows"
                    ));
                }
                let re = regex::Regex::new(regex).map_err(|e| format!("invalid regex: {e}"))?;
                let mut keep = HashSet::new();
                for (index, row) in rows.iter().enumerate() {
                    if re.is_match(&row.text) {
                        for selected in index.saturating_sub(*before)
                            ..=index
                                .saturating_add(*after)
                                .min(rows.len().saturating_sub(1))
                        {
                            keep.insert(selected);
                        }
                    }
                }
                rows = rows
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, r)| keep.contains(&i).then_some(r))
                    .collect()
            }
            Step::Exclude { regex } => rows = filter(rows, regex, false)?,
            Step::Head { n } => rows.truncate(*n),
            Step::Tail { n } => {
                let d = rows.len().saturating_sub(*n);
                rows.drain(..d);
            }
            Step::Unique => rows = unique(rows),
            Step::Count => return Ok(vec![rows.len().to_string()]),
            Step::GroupCount => {
                let mut counts = BTreeMap::new();
                for r in rows {
                    *counts.entry(r.text).or_insert(0_usize) += 1
                }
                return Ok(counts
                    .into_iter()
                    .map(|(v, n)| format!("{n}\t{v}"))
                    .collect());
            }
            Step::Sort { numeric } => {
                if *numeric {
                    rows.sort_by(|a, b| number(&a.text).total_cmp(&number(&b.text)))
                } else {
                    rows.sort_by(|a, b| a.text.cmp(&b.text))
                }
            }
            Step::Top { n } => rows.truncate(*n),
            Step::Sum => return Ok(vec![numbers(&rows)?.iter().sum::<f64>().to_string()]),
            Step::Min => {
                return Ok(vec![
                    numbers(&rows)?
                        .into_iter()
                        .reduce(f64::min)
                        .unwrap_or(f64::NAN)
                        .to_string(),
                ]);
            }
            Step::Max => {
                return Ok(vec![
                    numbers(&rows)?
                        .into_iter()
                        .reduce(f64::max)
                        .unwrap_or(f64::NAN)
                        .to_string(),
                ]);
            }
            Step::Mean => {
                let values = numbers(&rows)?;
                return Ok(vec![if values.is_empty() {
                    "NaN".into()
                } else {
                    (values.iter().sum::<f64>() / values.len() as f64).to_string()
                }]);
            }
            Step::JsonField { field } => {
                for r in &mut rows {
                    let value: serde_json::Value =
                        serde_json::from_str(&r.text).map_err(|e| format!("invalid JSONL: {e}"))?;
                    r.text = value.get(field).map(value_text).unwrap_or_default()
                }
            }
            Step::JsonEq { field, value } => rows.retain(|r| {
                serde_json::from_str::<serde_json::Value>(&r.text)
                    .ok()
                    .and_then(|v| v.get(field).cloned())
                    .as_ref()
                    == Some(value)
            }),
            Step::Column { index, delimiter } => {
                for r in &mut rows {
                    r.text = r
                        .text
                        .split(delimiter)
                        .nth(*index)
                        .unwrap_or("")
                        .to_string()
                }
            }
            Step::Stream { stream } => {
                let wanted = match stream.as_str() {
                    "stdout" => StreamKind::Stdout,
                    "stderr" => StreamKind::Stderr,
                    _ => return Err("stream must be stdout or stderr".into()),
                };
                rows.retain(|r| r.stream == wanted)
            }
            Step::Diagnostic => rows.retain(|r| diagnostic_re.is_match(&r.text)),
        }
    }
    Ok(rows.into_iter().map(|r| r.text).collect())
}
fn number(s: &str) -> f64 {
    s.trim().parse().unwrap_or(f64::NAN)
}
fn numbers(rows: &[Row]) -> Result<Vec<f64>, String> {
    rows.iter()
        .map(|r| {
            r.text
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("not numeric: {}", r.text))
        })
        .collect()
}
fn value_text(v: &serde_json::Value) -> String {
    v.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string())
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn reductions_are_terminal_and_preserve_final_results() {
        for (op, expected) in [
            ("count", "2"),
            ("group_count", "1\t1\n1\t3"),
            ("sum", "4"),
            ("min", "1"),
            ("max", "3"),
            ("mean", "2"),
        ] {
            let reduction =
                || serde_json::from_value::<Step>(serde_json::json!({"op": op})).unwrap();
            let rows = || {
                ["1", "3"]
                    .into_iter()
                    .map(|text| Row {
                        text: text.into(),
                        stream: StreamKind::Stdout,
                    })
                    .collect()
            };
            assert_eq!(apply(rows(), &[reduction()]).unwrap().join("\n"), expected);
            assert_eq!(
                apply(rows(), &[Step::Head { n: 2 }, reduction()])
                    .unwrap()
                    .join("\n"),
                expected
            );
            assert!(
                apply(rows(), &[reduction(), Step::Head { n: 0 }])
                    .unwrap_err()
                    .contains("terminal reduction")
            );
            assert!(apply(rows(), &[Step::Head { n: 1 }, reduction(), Step::Count]).is_err());
        }
        assert_eq!(apply(vec![], &[Step::Count]).unwrap(), ["0"]);
        // Rust f64 Sum starts at negative zero; preserve that pre-existing result.
        assert_eq!(apply(vec![], &[Step::Sum]).unwrap(), ["-0"]);
        assert_eq!(apply(vec![], &[Step::Mean]).unwrap(), ["NaN"]);
    }

    #[test]
    fn unique_bytes_charge_only_new_values_and_reject_before_insertion() {
        let mut seen = HashSet::new();
        let mut bytes = MAX_MATERIALIZED_BYTES - 3;
        assert!(retain_unique(&mut seen, &mut bytes, "abc").unwrap());
        assert_eq!(bytes, MAX_MATERIALIZED_BYTES);
        assert!(!retain_unique(&mut seen, &mut bytes, "abc").unwrap());
        assert!(retain_unique(&mut seen, &mut bytes, "d").is_err());
        assert_eq!(seen.len(), 1);
        assert_eq!(bytes, MAX_MATERIALIZED_BYTES);
    }

    #[test]
    fn materialization_preflight_rejects_before_copying() {
        assert!(check_materialization_budget(0, MAX_MATERIALIZED_BYTES as u64).is_ok());
        assert!(check_materialization_budget(0, MAX_MATERIALIZED_BYTES as u64 + 1).is_err());
        assert!(check_materialization_budget(MAX_MATERIALIZED_BYTES - 10, 11).is_err());
        assert!(check_materialization_budget(MAX_MATERIALIZED_BYTES - 10, 10).is_ok());
        assert!(check_materialization_budget(0, u64::MAX).is_err());
    }
}
