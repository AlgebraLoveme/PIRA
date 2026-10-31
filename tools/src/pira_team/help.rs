const OVERVIEW: &str = include_str!("help.md");
const COMMANDS: &str = include_str!("help_commands.md");
const MANUAL: &str = include_str!("help_manual.md");

pub(super) fn topic(name: &str) -> Option<&'static str> {
    let name = match name {
        "help" | "-h" | "--help" => return Some(OVERVIEW),
        "all" | "manual" | "--all" => return Some(MANUAL),
        "remote-keygen" | "remote-check" | "attach" => "remote",
        "config show" | "config set" | "config reset" => "config",
        _ => name,
    };
    COMMANDS.split("\n# ").find_map(|section| {
        let section = section.strip_prefix("# ").unwrap_or(section);
        let (heading, body) = section.split_once('\n')?;
        (heading == name).then_some(body)
    })
}

// Only explicit help shapes are intercepted. Runtime operands and option values,
// including help-looking task text and everything after --, stay with their parser.
pub(super) fn dispatch(args: &[String]) -> Option<Result<&'static str, String>> {
    let first = match args.first() {
        None => return Some(Ok(OVERVIEW)),
        Some(first) => first.as_str(),
    };
    let topics = if matches!(first, "help" | "--help" | "-h") {
        &args[1..]
    } else if first == "--help-all" {
        return Some(if args.len() == 1 {
            Ok(MANUAL)
        } else {
            Err("--help-all does not accept arguments; use pira_team help TOPIC".into())
        });
    } else if matches!(args, [_, flag] if matches!(flag.as_str(), "--help" | "-h")) {
        &args[..1]
    } else if matches!(args, [command, _, flag] if command == "config" && matches!(flag.as_str(), "--help" | "-h"))
    {
        &args[..2]
    } else {
        return None;
    };
    let name = match topics {
        [] => return Some(Ok(OVERVIEW)),
        [name] => name.clone(),
        [command, action] if command == "config" => format!("config {action}"),
        _ => {
            return Some(Err(
                "help accepts one topic or config subcommand; use pira_team --help".into(),
            ));
        }
    };
    Some(topic(&name).ok_or_else(|| format!("unknown help topic {name:?}; use pira_team --help")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_routing_leaves_runtime_arguments_untouched() {
        for args in [
            vec!["run", "--task", "--help"],
            vec!["run", "--task", "-h"],
            vec!["run", "--task=--help"],
            vec!["run", "--completion-gate", "--help"],
            vec!["run", "--label", "--help"],
            vec!["run", "--", "--help"],
            vec!["run", "--host", "--help"],
            vec!["run", "--host=--help"],
            vec!["run", "--task", "--host", "--help"],
            vec!["read", "RUN_ID", "--help"],
            vec!["resume", "RUN_ID", "--task", "--help"],
            vec!["search", "--", "--help"],
            vec!["config", "set", "--model", "--help"],
            vec!["--version"],
            vec!["--version", "extra"],
        ] {
            let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
            let before = args.clone();
            assert!(dispatch(&args).is_none(), "{args:?}");
            assert_eq!(args, before);
        }
    }
}
