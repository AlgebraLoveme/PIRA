//! CLI token boundaries and explicit help; commands retain validity and authority checks.
pub(crate) mod help;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Flag {
    InjectReview,
    InjectImplement,
    InjectFig,
    InjectWrite,
    AllowSubteam,
    AllowFix,
    CodeReview,
    Detach,
    Artifact,
    Recursive,
}

impl Flag {
    fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "--inject-review" => Self::InjectReview,
            "--inject-implement" => Self::InjectImplement,
            "--inject-fig" => Self::InjectFig,
            "--inject-write" => Self::InjectWrite,
            "--allow-subteam" => Self::AllowSubteam,
            "--allow-fix" => Self::AllowFix,
            "--code-review" => Self::CodeReview,
            "--detach" => Self::Detach,
            "--artifact" => Self::Artifact,
            "-r" | "--recursive" => Self::Recursive,
            _ => return None,
        })
    }
}

pub(crate) struct Token<'a> {
    pub index: usize,
    pub raw: &'a String,
    pub name: &'a str,
    pub value: Option<&'a str>,
    pub flag: Option<Flag>,
    pub words: &'a [String],
}

/// Keep original words/indices for forwarding. Unknown dashed options consume
/// one value; consumers decide whether an option or inline spelling is valid.
pub(crate) fn tokens(args: &[String]) -> impl Iterator<Item = Token<'_>> {
    let mut index = 0;
    let mut options = true;
    std::iter::from_fn(move || {
        let start = index;
        let raw = args.get(index)?;
        index += 1;
        let (mut name, mut value, mut flag) = (raw.as_str(), None, None);
        if options && raw == "--" {
            options = false;
        } else if options && raw.starts_with('-') {
            if let Some((key, inline)) = raw.split_once('=') {
                name = key;
                value = Some(inline);
            } else if let Some(boolean) = Flag::parse(raw) {
                flag = Some(boolean);
            } else if let Some(next) = args.get(index) {
                value = Some(next.as_str());
                index += 1;
            }
        }
        Some(Token {
            index: start,
            raw,
            name,
            value,
            flag,
            words: &args[start..index],
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boolean_arity_and_literal_boundaries_are_shared() {
        for boolean in [
            "--inject-review",
            "--inject-implement",
            "--inject-fig",
            "--inject-write",
            "--allow-subteam",
            "--allow-fix",
            "--code-review",
            "--detach",
            "--artifact",
            "-r",
            "--recursive",
        ] {
            let args = [
                boolean,
                "--task",
                "--allow-subteam",
                "--task=--host=literal",
                "--",
                "--inject-write",
                "--host=literal",
            ]
            .map(str::to_owned);
            let parsed: Vec<_> = tokens(&args).collect();
            assert_eq!(parsed.len(), 6, "{boolean}");
            assert!(parsed[0].flag.is_some());
            assert_eq!(parsed[1].value, Some("--allow-subteam"));
            assert_eq!(parsed[1].words, &args[1..3]);
            assert_eq!(parsed[2].name, "--task");
            assert_eq!(parsed[2].value, Some("--host=literal"));
            assert_eq!(parsed[3].index, 4);
            assert!(parsed[4].flag.is_none());
            assert!(parsed[5].value.is_none());
            assert_eq!(
                parsed
                    .iter()
                    .flat_map(|t| t.words.iter())
                    .collect::<Vec<_>>(),
                args.iter().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_terminator_used_as_a_value_does_not_end_options() {
        let args = ["--task", "--", "--inject-write", "--task=", "--model"].map(str::to_owned);
        let parsed: Vec<_> = tokens(&args).collect();
        assert_eq!(parsed[0].value, Some("--"));
        assert_eq!(parsed[1].flag, Some(Flag::InjectWrite));
        assert_eq!(parsed[2].value, Some(""));
        assert_eq!(parsed[3].value, None);
    }
}
