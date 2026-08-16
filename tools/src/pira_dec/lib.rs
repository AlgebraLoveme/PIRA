mod cli;
mod export;
mod model;
mod storage;
mod util;

use cli::{Command, SearchField};
use model::{DecisionRecord, DecisionView};
use regex::Regex;
use serde::Serialize;
use std::path::Path;

pub fn run() -> i32 {
    match real_main() {
        Ok(code) => code,
        Err(error) if error == util::BROKEN_PIPE => 0,
        Err(error) => {
            eprintln!("pira_dec: {error}");
            2
        }
    }
}

fn real_main() -> Result<i32, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = cli::parse_args(&args)?;
    let store = config.store_dir.as_deref();
    match config.command {
        Command::Help(topic) => {
            util::stdout_text(topic.text())?;
            Ok(0)
        }
        Command::Version => {
            util::stdout_line(&format!("pira_dec {}", env!("CARGO_PKG_VERSION")))?;
            Ok(0)
        }
        Command::Add(draft) => run_add(store, draft),
        Command::Show { id, json } => run_show(store, &id, json),
        Command::List {
            since,
            until,
            limit,
            json,
        } => run_list(store, since.as_deref(), until.as_deref(), limit, json),
        Command::Export {
            output,
            since,
            until,
            limit,
        } => run_export(store, &output, since.as_deref(), until.as_deref(), limit),
        Command::Search {
            field,
            pattern,
            since,
            until,
            limit,
            json,
        } => run_search(
            store,
            field,
            pattern.as_deref(),
            since.as_deref(),
            until.as_deref(),
            limit,
            json,
        ),
        Command::Forget { id, confirmed } => run_forget(store, &id, confirmed),
    }
}

fn run_export(
    store: Option<&Path>,
    output: &Path,
    since: Option<&str>,
    until: Option<&str>,
    limit: Option<usize>,
) -> Result<i32, String> {
    let (since_ms, until_ms) = parse_time_window("export", since, until)?;
    let (records, skipped) = load_records(store)?;
    let records: Vec<_> = records
        .into_iter()
        .filter(|record| time_matches(record, since_ms, until_ms))
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    let html = export::render(&records, skipped.len())?;
    util::write_private_new(output, html.as_bytes())?;
    util::stdout_line(&format!(
        "{} | {} {}",
        util::single_line_clip(&output.display().to_string(), 300),
        records.len(),
        if records.len() == 1 {
            "decision"
        } else {
            "decisions"
        }
    ))?;
    Ok(0)
}

fn run_add(store: Option<&Path>, draft: model::DecisionDraft) -> Result<i32, String> {
    let record = storage::add(store, draft)?;
    util::stdout_line(&format!(
        "{} | {}",
        record.id,
        util::single_line_clip(record.selected_text()?, 200)
    ))?;
    Ok(0)
}

fn run_show(store: Option<&Path>, id: &str, json: bool) -> Result<i32, String> {
    let layout = storage::Layout::current(store)?;
    let path = match storage::resolve(&layout, id, false)? {
        storage::Resolution::Missing => {
            eprintln!("pira_dec: no decision matches {id:?}");
            return Ok(1);
        }
        storage::Resolution::Ambiguous => {
            eprintln!("pira_dec: decision prefix is ambiguous: {id:?}");
            return Ok(1);
        }
        storage::Resolution::Found(path) => path,
    };
    let record = match storage::read_record(&path) {
        Ok(record) => record,
        Err(storage::ReadFailure::Vanished) => {
            eprintln!("pira_dec: decision vanished before it could be read");
            return Ok(1);
        }
        Err(storage::ReadFailure::Invalid(error)) => return Err(error),
    };
    if json {
        print_json(&record.view()?)?;
    } else {
        print_human_record(&record)?;
    }
    Ok(0)
}

#[derive(Serialize)]
struct SkippedRecord {
    filename: String,
    error: String,
}

#[derive(Serialize)]
struct SearchOutput {
    matches: Vec<DecisionView>,
    skipped_count: usize,
    skipped: Vec<SkippedRecord>,
}

#[derive(Debug, Serialize)]
struct ListDecision {
    id: String,
    timestamp: String,
    decision: String,
}

#[derive(Serialize)]
struct ListOutput {
    decisions: Vec<ListDecision>,
    skipped_count: usize,
    skipped: Vec<SkippedRecord>,
}

fn run_list(
    store: Option<&Path>,
    since: Option<&str>,
    until: Option<&str>,
    limit: usize,
    json: bool,
) -> Result<i32, String> {
    let (since_ms, until_ms) = parse_time_window("list", since, until)?;
    let (records, skipped) = load_records(store)?;
    let decisions = collect_list_decisions(records, since_ms, until_ms, limit)?;
    if json {
        let output = ListOutput {
            decisions,
            skipped_count: skipped.len(),
            skipped,
        };
        print_json(&output)?;
    } else {
        for decision in decisions {
            util::stdout_line(&format_list_row(&decision))?;
        }
    }
    Ok(0)
}

fn collect_list_decisions(
    records: Vec<DecisionRecord>,
    since_ms: Option<u64>,
    until_ms: Option<u64>,
    limit: usize,
) -> Result<Vec<ListDecision>, String> {
    let mut decisions = Vec::new();
    for record in records {
        if !time_matches(&record, since_ms, until_ms) {
            continue;
        }
        let timestamp = util::format_rfc3339(record.timestamp_ms)?;
        let decision = record.selected_text()?.to_string();
        decisions.push(ListDecision {
            id: record.id,
            timestamp,
            decision,
        });
        if decisions.len() == limit {
            break;
        }
    }
    Ok(decisions)
}

fn format_list_row(decision: &ListDecision) -> String {
    format!(
        "{} | {}",
        decision.id,
        util::single_line_clip(&decision.decision, 200)
    )
}

fn run_search(
    store: Option<&Path>,
    field: Option<SearchField>,
    pattern: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
    limit: usize,
    json: bool,
) -> Result<i32, String> {
    let expression = match (field, pattern) {
        (Some(field), Some(pattern)) => Some((
            field,
            Regex::new(pattern).map_err(|error| format!("invalid regex: {error}"))?,
        )),
        (None, None) => None,
        _ => return Err("search requires --field and --regex together".into()),
    };
    let (since_ms, until_ms) = parse_time_window("search", since, until)?;
    let (records, skipped) = load_records(store)?;
    let mut matches = Vec::new();
    for record in records {
        if !time_matches(&record, since_ms, until_ms) {
            continue;
        }
        let text_matches = match expression.as_ref() {
            Some((field, expression)) => record_matches(&record, *field, expression)?,
            None => true,
        };
        if text_matches {
            matches.push(record.view()?);
            if matches.len() == limit {
                break;
            }
        }
    }
    let found = !matches.is_empty();
    if json {
        let output = SearchOutput {
            matches,
            skipped_count: skipped.len(),
            skipped,
        };
        print_json(&output)?;
    } else {
        if !found {
            util::stdout_line(&format!(
                "decisions_matched=0 complete={}{}",
                usize::from(skipped.is_empty()),
                if skipped.is_empty() {
                    String::new()
                } else {
                    format!(" skipped={}", skipped.len())
                }
            ))?;
        }
        for record in matches {
            util::stdout_line(&format!(
                "{} | {} | {} | {}",
                record.id,
                record.timestamp,
                record.maker,
                util::single_line_clip(&record.decision_text, 200)
            ))?;
        }
    }
    Ok(if found { 0 } else { 1 })
}

fn parse_time_window(
    command: &str,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<(Option<u64>, Option<u64>), String> {
    let (since_ms, until_ms) = if since.is_some() || until.is_some() {
        let now_ms = util::now_ms()?;
        (
            since
                .map(|value| util::parse_time_bound(value, now_ms))
                .transpose()?,
            until
                .map(|value| util::parse_time_bound(value, now_ms))
                .transpose()?,
        )
    } else {
        (None, None)
    };
    if since_ms
        .zip(until_ms)
        .is_some_and(|(since, until)| since >= until)
    {
        return Err(format!("{command} --since must be earlier than --until"));
    }
    Ok((since_ms, until_ms))
}

fn load_records(store: Option<&Path>) -> Result<(Vec<DecisionRecord>, Vec<SkippedRecord>), String> {
    let layout = storage::Layout::current(store)?;
    let mut records = Vec::new();
    let mut skipped = Vec::new();
    for path in storage::record_paths(&layout)? {
        match storage::read_record(&path) {
            Ok(record) => records.push(record),
            Err(storage::ReadFailure::Vanished) => {}
            Err(storage::ReadFailure::Invalid(error)) => {
                let filename = filename_only(&path);
                eprintln!(
                    "pira_dec: skipped {}: {}",
                    filename,
                    util::single_line_clip(&error, 300)
                );
                skipped.push(SkippedRecord {
                    filename,
                    error: util::single_line_clip(&error, 300),
                });
            }
        }
    }
    sort_records_newest_first(&mut records);
    Ok((records, skipped))
}

fn sort_records_newest_first(records: &mut [DecisionRecord]) {
    records.sort_by(|left, right| {
        right
            .timestamp_ms
            .cmp(&left.timestamp_ms)
            .then_with(|| right.id.cmp(&left.id))
    });
}

fn time_matches(record: &DecisionRecord, since_ms: Option<u64>, until_ms: Option<u64>) -> bool {
    since_ms.is_none_or(|since| record.timestamp_ms >= since)
        && until_ms.is_none_or(|until| record.timestamp_ms < until)
}

fn run_forget(store: Option<&Path>, id: &str, confirmed: bool) -> Result<i32, String> {
    if !confirmed {
        return Err("forget requires explicit --yes".into());
    }
    model::validate_id_syntax(id)?;
    let layout = storage::Layout::current(store)?;
    match storage::delete_exact(&layout, id)? {
        Some(record) => {
            util::stdout_line(&format!(
                "{} | {}",
                record.id,
                util::single_line_clip(record.selected_text()?, 200)
            ))?;
            Ok(0)
        }
        None => {
            eprintln!("pira_dec: no exact decision {id:?}");
            Ok(1)
        }
    }
}

fn record_matches(
    record: &DecisionRecord,
    field: SearchField,
    regex: &Regex,
) -> Result<bool, String> {
    Ok(match field {
        SearchField::Id => regex.is_match(&record.id),
        SearchField::Context => regex.is_match(&record.context),
        SearchField::Choice => record.choices.iter().any(|choice| regex.is_match(choice)),
        SearchField::Decision => regex.is_match(record.selected_text()?),
        SearchField::Maker => regex.is_match(record.maker.as_str()),
        SearchField::Timestamp => regex.is_match(&util::format_rfc3339(record.timestamp_ms)?),
    })
}

fn print_human_record(record: &DecisionRecord) -> Result<(), String> {
    let mut output = String::new();
    output.push_str(&format!("ID: {}\n", record.id));
    output.push_str(&format!(
        "Timestamp: {} ({})\n",
        util::format_rfc3339(record.timestamp_ms)?,
        record.timestamp_ms
    ));
    output.push_str(&format!("Maker: {}\n", record.maker.as_str()));
    output.push_str("Context:\n");
    output.push_str(&record.context);
    output.push_str("\nChoices:\n");
    for (index, choice) in record.choices.iter().enumerate() {
        let selected = if index + 1 == record.decision as usize {
            " [selected]"
        } else {
            ""
        };
        output.push_str(&format!("  {}. {}{}\n", index + 1, choice, selected));
    }
    output.push_str(&format!(
        "Decision: {}. {}\n",
        record.decision,
        record.selected_text()?
    ));
    util::stdout_text(&output)
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    util::stdout_text(&json)
}

fn filename_only(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|value| util::single_line_clip(value, 200))
        .unwrap_or_else(|| "<invalid-filename>".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Maker;

    fn record(timestamp_ms: u64) -> DecisionRecord {
        DecisionRecord {
            id: format!("D-19700101T000000000Z-{timestamp_ms:016x}"),
            timestamp_ms,
            context: "Choose cache format".into(),
            choices: vec!["SQLite".into(), "JSON".into()],
            decision: 1,
            maker: Maker::Agent,
        }
    }

    #[test]
    fn time_window_is_since_inclusive_and_until_exclusive() {
        let record = record(2_000);
        assert!(time_matches(&record, Some(2_000), Some(2_001)));
        assert!(!time_matches(&record, Some(2_001), None));
        assert!(!time_matches(&record, None, Some(2_000)));
    }

    #[test]
    fn list_is_newest_first_bounded_and_concise() {
        let mut records = vec![record(1_000), record(3_000), record(2_000)];
        sort_records_newest_first(&mut records);
        let decisions = collect_list_decisions(records, Some(2_000), Some(4_000), 1).unwrap();
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].id.ends_with("0000000000000bb8"));

        let row = format_list_row(&decisions[0]);
        assert!(row.contains("SQLite"));
        assert_eq!(row.matches(" | ").count(), 1);
        assert!(!row.contains("Choose cache format"));
        assert!(!row.contains("agent"));
        let json = serde_json::to_value(&decisions[0]).unwrap();
        let keys = json.as_object().unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains_key("id"));
        assert!(keys.contains_key("timestamp"));
        assert!(keys.contains_key("decision"));
    }
}
