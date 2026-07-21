use std::io;
use std::path::Path;

use crate::language::Language;

pub type CommandError = (i32, String);
pub type CommandResult = Result<(), CommandError>;

pub fn input_error<T: Into<String>>(message: T) -> CommandError {
    (2, message.into())
}

pub fn lsp_error<T: Into<String>>(message: T) -> CommandError {
    (3, message.into())
}

pub fn output_error(error: io::Error) -> CommandError {
    if error.kind() == io::ErrorKind::BrokenPipe {
        (0, String::new())
    } else {
        (1, format!("cannot write output: {error}"))
    }
}

pub fn language_for(path: &Path, explicit: Option<Language>) -> Result<Language, CommandError> {
    let detected = Language::infer(path);
    match (explicit, detected) {
        (Some(explicit), Ok(detected)) if explicit != detected => Err((
            2,
            format!(
                "language mismatch: explicit {} but `{}` is {}",
                explicit.name(),
                path.display(),
                detected.name()
            ),
        )),
        (Some(explicit), _) => Ok(explicit),
        (None, Ok(detected)) => Ok(detected),
        (None, Err(error)) => Err((2, error)),
    }
}

pub fn positive_usize(value: &str, option: &str) -> Result<usize, CommandError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| (2, format!("{option} requires a positive integer")))
}

pub fn parse_location(value: &str) -> Option<(&str, usize, Option<usize>)> {
    let (prefix, last) = value.rsplit_once(':')?;
    let last_number = last.parse::<usize>().ok()?;
    if let Some((path, line)) = prefix.rsplit_once(':')
        && let Ok(line) = line.parse::<usize>()
    {
        return Some((path, line, Some(last_number)));
    }
    Some((prefix, last_number, None))
}
