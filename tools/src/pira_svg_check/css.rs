use std::iter::Peekable;
use std::str::Chars;

use crate::GuardError;

pub(super) fn validate_reference(value: &str) -> Result<(), GuardError> {
    let value = value.trim();
    if value.is_empty()
        || value.starts_with('#')
        || value
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
    {
        Ok(())
    } else {
        Err(GuardError(format!(
            "external SVG resource is not allowed: {value}"
        )))
    }
}

// Scan CSS tokens, not arbitrary XML strings. Strings and comments outside url()
// are inert; escapes must be decoded in both function names and URL arguments.
pub(super) fn validate_urls(value: &str) -> Result<(), GuardError> {
    let mut chars = value.chars().peekable();
    while chars.peek().is_some() {
        skip_space_and_comments(&mut chars);
        let Some(ch) = chars.next() else { break };
        match ch {
            '\'' | '"' => {
                string(&mut chars, ch)?;
            }
            '@' => {
                let name = name(&mut chars)?;
                if name.eq_ignore_ascii_case("import") {
                    return Err(GuardError(
                        "external CSS imports are not allowed".to_string(),
                    ));
                }
            }
            ch if is_name_char(ch) || ch == '\\' => {
                let mut identifier = String::new();
                if ch == '\\' {
                    if let Some(ch) = escape(&mut chars)? {
                        identifier.push(ch);
                    }
                } else {
                    identifier.push(ch);
                }
                identifier.push_str(&name(&mut chars)?);
                if identifier.eq_ignore_ascii_case("url") {
                    skip_space_and_comments(&mut chars);
                    if chars.next_if_eq(&'(').is_some() {
                        validate_reference(&url(&mut chars)?)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') || !ch.is_ascii()
}

fn name(chars: &mut Peekable<Chars<'_>>) -> Result<String, GuardError> {
    let mut result = String::new();
    while let Some(&ch) = chars.peek() {
        if ch == '\\' {
            chars.next();
            if let Some(ch) = escape(chars)? {
                result.push(ch);
            }
        } else if is_name_char(ch) {
            chars.next();
            result.push(ch);
        } else {
            break;
        }
    }
    Ok(result)
}

fn space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

fn skip_space_and_comments(chars: &mut Peekable<Chars<'_>>) {
    loop {
        while chars.next_if(|ch| space(*ch)).is_some() {}
        let mut ahead = chars.clone();
        if ahead.next() != Some('/') || ahead.next() != Some('*') {
            return;
        }
        *chars = ahead;
        while let Some(ch) = chars.next() {
            if ch == '*' && chars.next_if_eq(&'/').is_some() {
                break;
            }
        }
    }
}

fn escape(chars: &mut Peekable<Chars<'_>>) -> Result<Option<char>, GuardError> {
    let ch = chars.next().ok_or_else(invalid_css)?;
    if ch.is_ascii_hexdigit() {
        let mut value = ch.to_digit(16).unwrap();
        for _ in 1..6 {
            let Some(ch) = chars.next_if(|ch| ch.is_ascii_hexdigit()) else {
                break;
            };
            value = value * 16 + ch.to_digit(16).unwrap();
        }
        if chars.next_if(|ch| space(*ch)) == Some('\r') {
            chars.next_if_eq(&'\n');
        }
        Ok(Some(
            char::from_u32(value)
                .filter(|ch| *ch != '\0')
                .unwrap_or('\u{fffd}'),
        ))
    } else if matches!(ch, '\n' | '\r' | '\u{c}') {
        if ch == '\r' {
            chars.next_if_eq(&'\n');
        }
        Ok(None)
    } else {
        Ok(Some(ch))
    }
}

fn string(chars: &mut Peekable<Chars<'_>>, quote: char) -> Result<String, GuardError> {
    let mut result = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            ch if ch == quote => return Ok(result),
            '\\' => {
                if let Some(ch) = escape(chars)? {
                    result.push(ch);
                }
            }
            '\n' | '\r' | '\u{c}' => return Err(invalid_css()),
            ch => result.push(ch),
        }
    }
    Err(invalid_css())
}

fn url(chars: &mut Peekable<Chars<'_>>) -> Result<String, GuardError> {
    skip_space_and_comments(chars);
    let mut result = String::new();
    if let Some(quote) = chars.next_if(|ch| matches!(ch, '\'' | '"')) {
        result = string(chars, quote)?;
    } else {
        while let Some(&ch) = chars.peek() {
            if ch == ')' || space(ch) {
                break;
            }
            chars.next();
            match ch {
                '\'' | '"' | '(' => return Err(invalid_css()),
                '\\' => {
                    if let Some(ch) = escape(chars)? {
                        result.push(ch);
                    }
                }
                ch => result.push(ch),
            }
        }
    }
    skip_space_and_comments(chars);
    if chars.next() != Some(')') {
        return Err(invalid_css());
    }
    Ok(result)
}

fn invalid_css() -> GuardError {
    GuardError("invalid SVG CSS string or URL".to_string())
}

#[cfg(test)]
mod tests {
    use super::validate_urls;

    #[test]
    fn css_escapes_comments_and_strings_do_not_hide_external_urls() {
        for css in [
            r"fill: u\72l(https://example.invalid/a)",
            r"fill: URL('\68ttps://example.invalid/a')",
            "fill:url(/*comment*/ 'https://example.invalid/a')",
            "fill: url(#local); stroke: url(https://example.invalid/a)",
            r"@\000069mport 'external.css';",
        ] {
            assert!(
                validate_urls(css)
                    .unwrap_err()
                    .to_string()
                    .contains("external"),
                "{css}"
            );
        }
    }

    #[test]
    fn local_data_and_inert_css_tokens_are_accepted() {
        for css in [
            r"fill: u\72l('\23 local')",
            "fill: URL( /*comment*/ '#local' /*comment*/ )",
            r"fill: url(data:image/png;base64,AAAA)",
            "fill: url('DATA:image/svg+xml,<svg>(text)</svg>')",
            "/* url(external) @import */ fill: black",
            "content: 'url(external)'; fill: url(#local)",
        ] {
            assert!(validate_urls(css).is_ok(), "{css}");
        }
    }

    #[test]
    fn malformed_urls_fail_closed() {
        for css in [
            "url('https://example.invalid/a'",
            "url(https://example.invalid/a",
            "url('unterminated)",
        ] {
            assert!(validate_urls(css).is_err(), "{css}");
        }
    }
}
