use crate::model::{DecisionDraft, Maker};
use std::path::PathBuf;

const MAX_REGEX_BYTES: usize = 4 * 1024;

pub const HELP: &str = r#"pira_decision records and searches medium-level workspace decisions.

USAGE
  pira_decision add --context TEXT --choice TEXT --choice TEXT --decision N --maker human|agent [OPTIONS]
  pira_decision show ID [--json] [--store-dir PATH]
  pira_decision search --field FIELD --regex PATTERN [--limit N] [--json] [--store-dir PATH]
  pira_decision forget EXACT_ID --yes [--store-dir PATH]

SEARCH FIELDS
  id, context, choice, decision, maker, timestamp

OPTIONS
  --store-dir PATH  Override the per-user durable decision store.
  --json            Emit stable JSON for show or search.
  --limit N         Return 1..1000 matches; default 20.
  -h, --help        Show this help.
  -V, --version     Show the version.

Records are scoped to the nearest Git root, otherwise the current directory.
Search is lock-free and may omit a decision published concurrently.
"#;

#[derive(Clone, Copy, Debug)]
pub enum SearchField {
    Id,
    Context,
    Choice,
    Decision,
    Maker,
    Timestamp,
}

impl SearchField {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "id" => Ok(Self::Id),
            "context" => Ok(Self::Context),
            "choice" => Ok(Self::Choice),
            "decision" => Ok(Self::Decision),
            "maker" => Ok(Self::Maker),
            "timestamp" => Ok(Self::Timestamp),
            _ => Err(format!(
                "unknown search field {value:?}; expected id, context, choice, decision, maker, or timestamp"
            )),
        }
    }
}

#[derive(Debug)]
pub enum Command {
    Help,
    Version,
    Add(DecisionDraft),
    Show {
        id: String,
        json: bool,
    },
    Search {
        field: SearchField,
        pattern: String,
        limit: usize,
        json: bool,
    },
    Forget {
        id: String,
        confirmed: bool,
    },
}

#[derive(Debug)]
pub struct Config {
    pub command: Command,
    pub store_dir: Option<PathBuf>,
}

pub fn parse_args(args: &[String]) -> Result<Config, String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Config {
            command: Command::Help,
            store_dir: None,
        });
    };
    match command {
        "-h" | "--help" | "help" => simple(Command::Help, &args[1..]),
        "-V" | "--version" | "version" => simple(Command::Version, &args[1..]),
        "add" => parse_add(&args[1..]),
        "show" => parse_show(&args[1..]),
        "search" => parse_search(&args[1..]),
        "forget" => parse_forget(&args[1..]),
        _ => Err(format!(
            "unknown command {command:?}; run pira_decision --help"
        )),
    }
}

fn simple(command: Command, remaining: &[String]) -> Result<Config, String> {
    if !remaining.is_empty() {
        return Err("help and version do not accept arguments".into());
    }
    Ok(Config {
        command,
        store_dir: None,
    })
}

fn parse_add(args: &[String]) -> Result<Config, String> {
    if wants_help(args) {
        return simple(Command::Help, &[]);
    }
    let mut context = None;
    let mut choices = Vec::new();
    let mut decision = None;
    let mut makers = Vec::new();
    let mut store_dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--context" => set_once(
                &mut context,
                value(args, &mut index, "--context")?,
                "--context",
            )?,
            "--choice" => choices.push(value(args, &mut index, "--choice")?),
            "--decision" => {
                let raw = value(args, &mut index, "--decision")?;
                let parsed = raw
                    .parse::<u32>()
                    .map_err(|_| "--decision must be an unsigned integer".to_string())?;
                set_once(&mut decision, parsed, "--decision")?;
            }
            "--maker" => makers.push(Maker::parse(&value(args, &mut index, "--maker")?)?),
            "--store-dir" => {
                let path = PathBuf::from(value(args, &mut index, "--store-dir")?);
                set_once(&mut store_dir, path, "--store-dir")?;
            }
            other => return Err(format!("unknown add argument {other:?}")),
        }
        index += 1;
    }
    Ok(Config {
        command: Command::Add(DecisionDraft {
            context: context.ok_or_else(|| "add requires --context".to_string())?,
            choices,
            decision: decision.ok_or_else(|| "add requires --decision".to_string())?,
            makers,
        }),
        store_dir,
    })
}

fn parse_show(args: &[String]) -> Result<Config, String> {
    if wants_help(args) {
        return simple(Command::Help, &[]);
    }
    let mut id = None;
    let mut json = false;
    let mut store_dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            "--store-dir" => {
                let path = PathBuf::from(value(args, &mut index, "--store-dir")?);
                set_once(&mut store_dir, path, "--store-dir")?;
            }
            other if !other.starts_with('-') => set_once(&mut id, other.to_string(), "ID")?,
            other => return Err(format!("unknown show argument {other:?}")),
        }
        index += 1;
    }
    Ok(Config {
        command: Command::Show {
            id: id.ok_or_else(|| "show requires an ID or prefix".to_string())?,
            json,
        },
        store_dir,
    })
}

fn parse_search(args: &[String]) -> Result<Config, String> {
    if wants_help(args) {
        return simple(Command::Help, &[]);
    }
    let mut field = None;
    let mut pattern = None;
    let mut limit = 20_usize;
    let mut limit_set = false;
    let mut json = false;
    let mut store_dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--field" => {
                let parsed = SearchField::parse(&value(args, &mut index, "--field")?)?;
                set_once(&mut field, parsed, "--field")?;
            }
            "--regex" => {
                let raw = value(args, &mut index, "--regex")?;
                if raw.len() > MAX_REGEX_BYTES {
                    return Err(format!("--regex exceeds {MAX_REGEX_BYTES} UTF-8 bytes"));
                }
                set_once(&mut pattern, raw, "--regex")?;
            }
            "--limit" => {
                if limit_set {
                    return Err("--limit may appear only once".into());
                }
                let raw = value(args, &mut index, "--limit")?;
                limit = raw
                    .parse::<usize>()
                    .map_err(|_| "--limit must be an integer from 1 through 1000".to_string())?;
                if !(1..=1_000).contains(&limit) {
                    return Err("--limit must be from 1 through 1000".into());
                }
                limit_set = true;
            }
            "--json" => json = true,
            "--store-dir" => {
                let path = PathBuf::from(value(args, &mut index, "--store-dir")?);
                set_once(&mut store_dir, path, "--store-dir")?;
            }
            other => return Err(format!("unknown search argument {other:?}")),
        }
        index += 1;
    }
    Ok(Config {
        command: Command::Search {
            field: field.ok_or_else(|| "search requires --field".to_string())?,
            pattern: pattern.ok_or_else(|| "search requires --regex".to_string())?,
            limit,
            json,
        },
        store_dir,
    })
}

fn parse_forget(args: &[String]) -> Result<Config, String> {
    if wants_help(args) {
        return simple(Command::Help, &[]);
    }
    let mut id = None;
    let mut confirmed = false;
    let mut store_dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--yes" => confirmed = true,
            "--store-dir" => {
                let path = PathBuf::from(value(args, &mut index, "--store-dir")?);
                set_once(&mut store_dir, path, "--store-dir")?;
            }
            other if !other.starts_with('-') => set_once(&mut id, other.to_string(), "ID")?,
            other => return Err(format!("unknown forget argument {other:?}")),
        }
        index += 1;
    }
    Ok(Config {
        command: Command::Forget {
            id: id.ok_or_else(|| "forget requires an exact ID".to_string())?,
            confirmed,
        },
        store_dir,
    })
}

fn value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn set_once<T>(slot: &mut Option<T>, value: T, label: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{label} may appear only once"));
    }
    Ok(())
}

fn wants_help(args: &[String]) -> bool {
    matches!(args, [value] if value == "-h" || value == "--help")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parses_add_with_joint_makers() {
        let config = parse_args(&args(&[
            "add",
            "--context",
            "Choose storage",
            "--choice",
            "SQL",
            "--choice",
            "Files",
            "--decision",
            "2",
            "--maker",
            "human",
            "--maker",
            "agent",
        ]))
        .unwrap();
        let Command::Add(draft) = config.command else {
            panic!("expected add command");
        };
        assert_eq!(draft.choices.len(), 2);
        assert_eq!(draft.decision, 2);
        assert_eq!(draft.makers.len(), 2);
    }

    #[test]
    fn rejects_out_of_range_search_limit() {
        let error = parse_args(&args(&[
            "search", "--field", "context", "--regex", "x", "--limit", "0",
        ]))
        .unwrap_err();
        assert!(error.contains("1 through 1000"));
    }
}
