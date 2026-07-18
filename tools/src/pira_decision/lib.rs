mod cli;
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
            eprintln!("pira_decision: {error}");
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
            util::stdout_line(&format!("pira_decision {}", env!("CARGO_PKG_VERSION")))?;
            Ok(0)
        }
        Command::Add(draft) => run_add(store, draft),
        Command::Show { id, json } => run_show(store, &id, json),
        Command::Search {
            field,
            pattern,
            limit,
            json,
        } => run_search(store, field, &pattern, limit, json),
        Command::Forget { id, confirmed } => run_forget(store, &id, confirmed),
    }
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
            eprintln!("pira_decision: no decision matches {id:?}");
            return Ok(1);
        }
        storage::Resolution::Ambiguous => {
            eprintln!("pira_decision: decision prefix is ambiguous: {id:?}");
            return Ok(1);
        }
        storage::Resolution::Found(path) => path,
    };
    let record = match storage::read_record(&path) {
        Ok(record) => record,
        Err(storage::ReadFailure::Vanished) => {
            eprintln!("pira_decision: decision vanished before it could be read");
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

fn run_search(
    store: Option<&Path>,
    field: SearchField,
    pattern: &str,
    limit: usize,
    json: bool,
) -> Result<i32, String> {
    let expression = Regex::new(pattern).map_err(|error| format!("invalid regex: {error}"))?;
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
                    "pira_decision: skipped {}: {}",
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
    records.sort_by(|left, right| {
        right
            .timestamp_ms
            .cmp(&left.timestamp_ms)
            .then_with(|| right.id.cmp(&left.id))
    });
    let mut matches = Vec::new();
    for record in records {
        if record_matches(&record, field, &expression)? {
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
        for record in matches {
            util::stdout_line(&format!(
                "{} | {} | {} | {}",
                record.id,
                record.timestamp,
                record.makers.join(","),
                util::single_line_clip(&record.decision_text, 200)
            ))?;
        }
    }
    Ok(if found { 0 } else { 1 })
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
            eprintln!("pira_decision: no exact decision {id:?}");
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
        SearchField::Maker => record
            .makers
            .iter()
            .any(|maker| regex.is_match(maker.as_str())),
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
    output.push_str(&format!(
        "Makers: {}\n",
        record
            .makers
            .iter()
            .map(|maker| maker.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
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
