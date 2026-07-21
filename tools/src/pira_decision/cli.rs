use crate::model::{DecisionDraft, Maker};
use std::path::PathBuf;

const MAX_REGEX_BYTES: usize = 4 * 1024;
const MAX_TIME_BYTES: usize = 256;

pub const HELP: &str = r#"pira_decision records and searches medium-level workspace decisions.

USAGE
  pira_decision add --context TEXT --choice TEXT --choice TEXT --decision N --maker human|agent [OPTIONS]
  pira_decision show ID [--json] [--store-dir PATH]
  pira_decision search [--field FIELD --regex PATTERN] [--since TIME] [--until TIME]
                       [--limit N] [--json] [--store-dir PATH]
  pira_decision forget EXACT_ID --yes [--store-dir PATH]
  pira_decision help [COMMAND]

Records are scoped to the nearest Git root, otherwise the current directory.
Search is lock-free and may omit a decision published concurrently.
Run `pira_decision COMMAND --help` for exact fields, options, and behavior.
"#;

const ADD_HELP: &str = r#"pira_decision add — record one concluded workspace decision

USAGE
  pira_decision add --context TEXT --choice TEXT --choice TEXT --decision N --maker human|agent [OPTIONS]

FIELDS
  --context TEXT    Concise problem and decisive constraints; exactly once.
  --choice TEXT     Seriously considered alternative; repeat for two or more unique choices.
  --decision N      One-based index selecting one listed choice.
  --maker VALUE     Decision authority: human or agent; human overrides agent if repeated.

OPTIONS
  --store-dir PATH  Override the durable per-user store.
  -h, --help        Show this help.
"#;

const SHOW_HELP: &str = r#"pira_decision show — display one validated decision record

USAGE
  pira_decision show ID [--json] [--store-dir PATH]

ID may be complete or an unambiguous prefix. The requested record is integrity-checked before
display. Use --json for stable programmatic output.
"#;

const SEARCH_HELP: &str = r#"pira_decision search — filter workspace decisions

USAGE
  pira_decision search [--field FIELD --regex PATTERN] [--since TIME] [--until TIME]
                       [--limit N] [--json] [--store-dir PATH]

FIELDS
  id         Generated decision ID.
  context    Decision context.
  choice     Every considered alternative.
  decision   Selected choice text only.
  maker      human or agent.
  timestamp  RFC 3339 UTC timestamp.

TIME is RFC 3339, `now`, or an age such as 30m, 24h, or 7d. --since includes records at or after its
bound; --until excludes records at or after its bound. Use either a field/regex pair, a time bound,
or both. Regex matching is case-sensitive unless PATTERN enables a flag such as (?i). Results are
newest first; --limit accepts 1..1000 and defaults to 20. Search skips unrelated invalid records,
reports them as warnings, and may omit a record published concurrently. Use --json for structured
matches and skipped-record details.

EXAMPLES
  pira_decision search --since 7d --limit 20
  pira_decision search --field context --regex '(?i)cache' --since 30d
"#;

const FORGET_HELP: &str = r#"pira_decision forget — logically delete one exact decision record

USAGE
  pira_decision forget EXACT_ID --yes [--store-dir PATH]

The complete ID and explicit --yes are required. Deletion removes the managed record but does not
claim secure physical erasure.
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HelpTopic {
    Global,
    Add,
    Show,
    Search,
    Forget,
}

impl HelpTopic {
    pub fn text(self) -> &'static str {
        match self {
            Self::Global => HELP,
            Self::Add => ADD_HELP,
            Self::Show => SHOW_HELP,
            Self::Search => SEARCH_HELP,
            Self::Forget => FORGET_HELP,
        }
    }
}

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
    Help(HelpTopic),
    Version,
    Add(DecisionDraft),
    Show {
        id: String,
        json: bool,
    },
    Search {
        field: Option<SearchField>,
        pattern: Option<String>,
        since: Option<String>,
        until: Option<String>,
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
            command: Command::Help(HelpTopic::Global),
            store_dir: None,
        });
    };
    match command {
        "-h" | "--help" => simple(Command::Help(HelpTopic::Global), &args[1..]),
        "help" => parse_help(&args[1..]),
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

fn parse_help(args: &[String]) -> Result<Config, String> {
    let topic = match args {
        [] => HelpTopic::Global,
        [command] => match command.as_str() {
            "add" => HelpTopic::Add,
            "show" => HelpTopic::Show,
            "search" => HelpTopic::Search,
            "forget" => HelpTopic::Forget,
            _ => return Err(format!("unknown help command {command:?}")),
        },
        _ => return Err("help accepts at most one command".into()),
    };
    simple(Command::Help(topic), &[])
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
        return simple(Command::Help(HelpTopic::Add), &[]);
    }
    let mut context = None;
    let mut choices = Vec::new();
    let mut decision = None;
    let mut maker = None;
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
            "--maker" => {
                let next = Maker::parse(&value(args, &mut index, "--maker")?)?;
                maker = Some(if maker == Some(Maker::Human) || next == Maker::Human {
                    Maker::Human
                } else {
                    Maker::Agent
                });
            }
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
            maker: maker.ok_or_else(|| "add requires --maker".to_string())?,
        }),
        store_dir,
    })
}

fn parse_show(args: &[String]) -> Result<Config, String> {
    if wants_help(args) {
        return simple(Command::Help(HelpTopic::Show), &[]);
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
        return simple(Command::Help(HelpTopic::Search), &[]);
    }
    let mut field = None;
    let mut pattern = None;
    let mut since = None;
    let mut until = None;
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
            "--since" => {
                let raw = value(args, &mut index, "--since")?;
                if raw.len() > MAX_TIME_BYTES {
                    return Err(format!("--since exceeds {MAX_TIME_BYTES} UTF-8 bytes"));
                }
                set_once(&mut since, raw, "--since")?;
            }
            "--until" => {
                let raw = value(args, &mut index, "--until")?;
                if raw.len() > MAX_TIME_BYTES {
                    return Err(format!("--until exceeds {MAX_TIME_BYTES} UTF-8 bytes"));
                }
                set_once(&mut until, raw, "--until")?;
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
    if field.is_some() != pattern.is_some() {
        return Err("search requires --field and --regex together".into());
    }
    if field.is_none() && since.is_none() && until.is_none() {
        return Err("search requires --field with --regex, --since, or --until".into());
    }
    Ok(Config {
        command: Command::Search {
            field,
            pattern,
            since,
            until,
            limit,
            json,
        },
        store_dir,
    })
}

fn parse_forget(args: &[String]) -> Result<Config, String> {
    if wants_help(args) {
        return simple(Command::Help(HelpTopic::Forget), &[]);
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
    fn human_maker_overrides_agent() {
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
        assert_eq!(draft.maker, Maker::Human);
    }

    #[test]
    fn rejects_out_of_range_search_limit() {
        let error = parse_args(&args(&[
            "search", "--field", "context", "--regex", "x", "--limit", "0",
        ]))
        .unwrap_err();
        assert!(error.contains("1 through 1000"));
    }

    #[test]
    fn parses_time_only_search() {
        let config = parse_args(&args(&[
            "search", "--since", "7d", "--until", "now", "--limit", "5",
        ]))
        .unwrap();
        let Command::Search {
            field,
            pattern,
            since,
            until,
            limit,
            ..
        } = config.command
        else {
            panic!("expected search command");
        };
        assert!(field.is_none());
        assert!(pattern.is_none());
        assert_eq!(since.as_deref(), Some("7d"));
        assert_eq!(until.as_deref(), Some("now"));
        assert_eq!(limit, 5);
    }

    #[test]
    fn search_requires_field_and_regex_together() {
        let error =
            parse_args(&args(&["search", "--field", "context", "--since", "7d"])).unwrap_err();
        assert!(error.contains("--field and --regex together"));
    }

    #[test]
    fn search_requires_at_least_one_filter() {
        let error = parse_args(&args(&["search", "--limit", "5"])).unwrap_err();
        assert!(error.contains("--since, or --until"));
    }

    #[test]
    fn rejects_oversized_search_time() {
        let oversized = "1".repeat(MAX_TIME_BYTES + 1);
        let error = parse_args(&["search".into(), "--since".into(), oversized]).unwrap_err();
        assert!(error.contains("--since exceeds"));
    }

    #[test]
    fn subcommand_help_selects_specific_topic() {
        let config = parse_args(&args(&["search", "--help"])).expect("parse help");
        assert!(matches!(config.command, Command::Help(HelpTopic::Search)));
        assert!(
            HelpTopic::Search
                .text()
                .contains("Selected choice text only")
        );

        let config = parse_args(&args(&["help", "add"])).expect("parse help alias");
        assert!(matches!(config.command, Command::Help(HelpTopic::Add)));
    }
}
