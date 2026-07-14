//! Length-preserving parser views for recently added language syntax that the
//! published Tree-sitter grammars do not yet accept.
//!
//! These views never change the source returned to callers.  A candidate is
//! used only when reparsing it reduces navigation-relevant parser defects.

/// Go 1.26 permits value expressions as the argument to `new`.  The current
/// grammar gives `new` a special type-first argument production.  Named types
/// and arbitrary value expressions are both valid in an ordinary call-shaped
/// parse, so rename the callee to another three-byte identifier unless its
/// operand starts with a type-literal form that only the special production can
/// parse (`[]T`, `map[K]V`, `struct {...}`, and similar).
pub fn go_new_value_parser_view(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    let mut output = bytes.to_vec();
    let mut changed = false;
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let Some((start, end)) = next_code_identifier(bytes, index) else {
            break;
        };
        index = end;
        if &bytes[start..end] != b"new" {
            continue;
        }
        let Some(open) = skip_ascii_space(bytes, end).filter(|&at| bytes[at] == b'(') else {
            continue;
        };
        let Some(argument) = skip_ascii_space(bytes, open + 1) else {
            continue;
        };
        if !is_go_type_literal_start(bytes, argument) {
            output[start..end].copy_from_slice(b"ptr");
            changed = true;
        }
    }
    changed.then(|| String::from_utf8(output).expect("source parser view remains UTF-8"))
}

/// TypeScript supports `in`, `out`, and `in out` variance annotations on type
/// parameters.  The current grammar does not.  Mask annotation tokens only in
/// the syntactic slot immediately after `<` or `,` and before a parameter name.
///
/// It also rejects reserved/contextual keywords as labeled-tuple element
/// names.  Such labels are unambiguous when a keyword immediately after `[` or
/// `,` is followed by `:` (or `?:`), so replace only the parser-view spelling
/// with an equal-length identifier.
pub fn typescript_recent_syntax_parser_view(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    let mut output = bytes.to_vec();
    let tokens = code_tokens(bytes);
    let mut changed = false;

    for window_start in 0..tokens.len() {
        let (start, end) = tokens[window_start];
        if !matches!(&bytes[start..end], b"in" | b"out") {
            continue;
        }
        let Some(previous) = window_start.checked_sub(1).map(|at| tokens[at]) else {
            continue;
        };
        if !matches!(&bytes[previous.0..previous.1], b"<" | b",") {
            continue;
        }
        let mut next = window_start + 1;
        if &bytes[start..end] == b"in"
            && tokens
                .get(next)
                .is_some_and(|&(left, right)| &bytes[left..right] == b"out")
        {
            blank_non_newlines(&mut output, tokens[next].0, tokens[next].1);
            next += 1;
        }
        if tokens
            .get(next)
            .is_some_and(|&(left, right)| is_identifier(&bytes[left..right]))
        {
            blank_non_newlines(&mut output, start, end);
            changed = true;
        }
    }

    for at in 0..tokens.len() {
        let (start, end) = tokens[at];
        if !is_typescript_keyword(&bytes[start..end]) {
            continue;
        }
        let Some(previous) = at.checked_sub(1).map(|slot| tokens[slot]) else {
            continue;
        };
        if !matches!(&bytes[previous.0..previous.1], b"[" | b",") {
            continue;
        }
        let Some(&(next_start, next_end)) = tokens.get(at + 1) else {
            continue;
        };
        let next = &bytes[next_start..next_end];
        let labeled = next == b":"
            || (next == b"?"
                && tokens
                    .get(at + 2)
                    .is_some_and(|&(left, right)| &bytes[left..right] == b":"));
        if labeled {
            output[start] = b'x';
            output[start + 1..end].fill(b'_');
            changed = true;
        }
    }

    changed.then(|| String::from_utf8(output).expect("source parser view remains UTF-8"))
}

/// C# 13 added the `allows ref struct` anti-constraint.  Model it as an
/// ordinary equal-length type constraint in the parser view.  This works in
/// both first and comma-separated constraint positions and cannot affect the
/// declarations exposed from the original source.
pub fn csharp_recent_syntax_parser_view(source: &str) -> Option<String> {
    const ORIGINAL: &[u8] = b"allows ref struct";
    const REPLACEMENT: &[u8] = b"PiraConstraint___";
    debug_assert_eq!(ORIGINAL.len(), REPLACEMENT.len());

    let bytes = source.as_bytes();
    let mut output = bytes.to_vec();
    let tokens = code_tokens(bytes);
    let mut changed = false;
    for window in tokens.windows(3) {
        let [allows, reference, structure] = window else {
            continue;
        };
        if &bytes[allows.0..allows.1] == b"allows"
            && &bytes[reference.0..reference.1] == b"ref"
            && &bytes[structure.0..structure.1] == b"struct"
        {
            let start = allows.0;
            let end = structure.1;
            blank_non_newlines(&mut output, start, end);
            output[start..start + REPLACEMENT.len()].copy_from_slice(REPLACEMENT);
            changed = true;
        }
    }

    // Ref assignments and ref-return expressions are modern C# expression
    // modifiers.  Masking only the modifier leaves the complete expression
    // shape available to a grammar that predates these slots.
    for at in 0..tokens.len() {
        let (start, end) = tokens[at];
        if &bytes[start..end] != b"ref" || at == 0 {
            continue;
        }
        let previous = tokens[at - 1];
        let follows_assignment = &bytes[previous.0..previous.1] == b"=";
        let follows_arrow = &bytes[previous.0..previous.1] == b">" && at >= 2 && {
            let before = tokens[at - 2];
            &bytes[before.0..before.1] == b"="
        };
        let follows_return = &bytes[previous.0..previous.1] == b"return";
        if follows_assignment || follows_arrow || follows_return {
            blank_non_newlines(&mut output, start, end);
            changed = true;
        }
    }

    // Some grammar versions misparse the standard unsafe reinterpretation
    // idiom `*(Type*)&value` and lose every following declaration.  Replace
    // only that tightly shaped expression with the C# default literal in the
    // parser view; arbitrary pointer expressions are left untouched.
    for at in 0..tokens.len() {
        if &bytes[tokens[at].0..tokens[at].1] != b"*"
            || tokens
                .get(at + 1)
                .is_none_or(|&(left, right)| &bytes[left..right] != b"(")
        {
            continue;
        }
        let mut depth = 0usize;
        let mut close = None;
        for (offset, &(left, right)) in tokens[at + 1..].iter().enumerate() {
            match &bytes[left..right] {
                b"(" => depth += 1,
                b")" => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(at + 1 + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        let type_start = tokens[at + 1].1;
        let type_end = tokens[close].0;
        if !is_csharp_type_fragment(bytes, type_start, type_end) {
            continue;
        }
        let after_close = close + 1;
        let operand_slot = if tokens
            .get(after_close)
            .is_some_and(|&(left, right)| &bytes[left..right] == b"&")
        {
            after_close + 1
        } else if bytes[type_start..type_end]
            .iter()
            .rev()
            .find(|byte| !byte.is_ascii_whitespace())
            == Some(&b'*')
        {
            after_close
        } else {
            continue;
        };
        if tokens
            .get(operand_slot)
            .is_none_or(|&(left, right)| !is_identifier(&bytes[left..right]))
        {
            continue;
        }
        let end = tokens[operand_slot].1;
        if end - tokens[at].0 >= b"default".len() {
            blank_non_newlines(&mut output, tokens[at].0, end);
            output[tokens[at].0..tokens[at].0 + b"default".len()].copy_from_slice(b"default");
            changed = true;
        }
    }

    // C# 14 extension blocks are declaration containers.  Present their body
    // to the older grammar as a nested class; the symbol walker recognizes the
    // original `extension` token at this exact range and flattens the synthetic
    // container, so no invented name reaches output.
    for at in 0..tokens.len() {
        let (start, end) = tokens[at];
        if &bytes[start..end] != b"extension" || !line_prefix_is_space(bytes, start) {
            continue;
        }
        let Some(open) = tokens[at + 1..]
            .iter()
            .position(|&(left, right)| &bytes[left..right] == b"(")
            .map(|offset| at + 1 + offset)
        else {
            continue;
        };
        if tokens[at + 1..open].iter().any(|&(left, right)| {
            !matches!(&bytes[left..right], b"<" | b">" | b",")
                && !is_identifier(&bytes[left..right])
        }) {
            continue;
        }
        let Some(close) = matching_token_delimiter(bytes, &tokens, open, b"(", b")") else {
            continue;
        };
        let Some(body) = tokens[close + 1..]
            .iter()
            .position(|&(left, right)| matches!(&bytes[left..right], b"{" | b";" | b"="))
            .map(|offset| close + 1 + offset)
        else {
            continue;
        };
        if &bytes[tokens[body].0..tokens[body].1] != b"{" || tokens[body].0 - start < 7 {
            continue;
        }
        blank_non_newlines(&mut output, start, tokens[body].0);
        output[start..start + 7].copy_from_slice(b"class E");
        changed = true;
    }

    // C# 14 permits null-conditional access on an assignment left-hand side.
    // Mask only the `?` when the same statement reaches an assignment before
    // its terminator; ordinary null-conditional reads remain unchanged.
    for at in 0..tokens.len().saturating_sub(1) {
        let (start, end) = tokens[at];
        if &bytes[start..end] != b"?"
            || !matches!(&bytes[tokens[at + 1].0..tokens[at + 1].1], b"." | b"[")
        {
            continue;
        }
        let assignment = tokens[at + 2..].iter().find_map(|&(left, right)| {
            let token = &bytes[left..right];
            if token == b"=" {
                Some(true)
            } else if matches!(token, b";" | b"{" | b"}") {
                Some(false)
            } else {
                None
            }
        });
        if assignment == Some(true) {
            blank_non_newlines(&mut output, start, end);
            changed = true;
        }
    }
    changed.then(|| String::from_utf8(output).expect("source parser view remains UTF-8"))
}

fn is_csharp_type_fragment(source: &[u8], start: usize, end: usize) -> bool {
    start < end
        && source[start..end].iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || byte.is_ascii_whitespace()
                || matches!(
                    *byte,
                    b'_' | b'.' | b'$' | b'<' | b'>' | b',' | b'?' | b'[' | b']' | b'*' | b':'
                )
                || *byte >= 0x80
        })
}

fn matching_token_delimiter(
    source: &[u8],
    tokens: &[(usize, usize)],
    open: usize,
    opening: &[u8],
    closing: &[u8],
) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, &(left, right)) in tokens[open..].iter().enumerate() {
        let token = &source[left..right];
        if token == opening {
            depth += 1;
        } else if token == closing {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(open + offset);
            }
        }
    }
    None
}

fn line_prefix_is_space(source: &[u8], start: usize) -> bool {
    let line = source[..start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    source[line..start].iter().all(u8::is_ascii_whitespace)
}

fn is_go_type_literal_start(source: &[u8], at: usize) -> bool {
    if source.get(at) == Some(&b'[') {
        return true;
    }
    if source.get(at..at + 2).is_some_and(|prefix| prefix == b"<-") {
        return true;
    }
    if source
        .get(at)
        .is_some_and(|byte| is_identifier_start(*byte))
    {
        let end = identifier_end(source, at);
        return matches!(
            &source[at..end],
            b"map" | b"chan" | b"func" | b"struct" | b"interface"
        );
    }
    false
}

fn code_tokens(source: &[u8]) -> Vec<(usize, usize)> {
    let mut tokens = Vec::with_capacity(source.len() / 6);
    let mut index = 0;
    while index < source.len() {
        match source[index] {
            byte if byte.is_ascii_whitespace() => index += 1,
            b'/' if source.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < source.len() && source[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if source.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < source.len()
                    && !(source[index] == b'*' && source[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(source.len());
            }
            quote @ (b'"' | b'\'' | b'`') => {
                index = quoted_end(source, index, quote, true);
            }
            byte if is_identifier_start(byte) => {
                let end = identifier_end(source, index);
                tokens.push((index, end));
                index = end;
            }
            _ => {
                tokens.push((index, index + 1));
                index += 1;
            }
        }
    }
    tokens
}

fn next_code_identifier(source: &[u8], mut index: usize) -> Option<(usize, usize)> {
    while index < source.len() {
        match source[index] {
            b'/' if source.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < source.len() && source[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if source.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < source.len()
                    && !(source[index] == b'*' && source[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(source.len());
            }
            quote @ (b'"' | b'\'' | b'`') => {
                index = quoted_end(source, index, quote, quote != b'`')
            }
            byte if is_identifier_start(byte) => {
                return Some((index, identifier_end(source, index)));
            }
            _ => index += 1,
        }
    }
    None
}

fn quoted_end(source: &[u8], start: usize, quote: u8, backslash_escapes: bool) -> usize {
    let quote_run = if quote == b'"' {
        source[start..]
            .iter()
            .take_while(|byte| **byte == quote)
            .count()
    } else {
        1
    };
    if quote_run >= 3 {
        let mut index = start + quote_run;
        while index < source.len() {
            if source[index] == quote
                && source[index..]
                    .iter()
                    .take_while(|byte| **byte == quote)
                    .count()
                    >= quote_run
            {
                return index + quote_run;
            }
            index += 1;
        }
        return source.len();
    }

    let verbatim = quote == b'"'
        && (source.get(start.wrapping_sub(1)) == Some(&b'@')
            || (start >= 2 && matches!(&source[start - 2..start], b"@$" | b"$@")));
    let mut index = start + 1;
    while index < source.len() {
        if verbatim && source[index] == quote && source.get(index + 1) == Some(&quote) {
            index += 2;
        } else if backslash_escapes && source[index] == b'\\' {
            index = (index + 2).min(source.len());
        } else if source[index] == quote {
            return index + 1;
        } else {
            index += 1;
        }
    }
    source.len()
}

fn skip_ascii_space(source: &[u8], mut index: usize) -> Option<usize> {
    while source.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    (index < source.len()).then_some(index)
}

fn identifier_end(source: &[u8], mut index: usize) -> usize {
    while source
        .get(index)
        .is_some_and(|byte| is_identifier_continue(*byte))
    {
        index += 1;
    }
    index
}

fn is_identifier(source: &[u8]) -> bool {
    source
        .first()
        .is_some_and(|byte| is_identifier_start(*byte))
        && source[1..].iter().all(|byte| is_identifier_continue(*byte))
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' || byte >= 0x80
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn blank_non_newlines(output: &mut [u8], start: usize, end: usize) {
    for byte in &mut output[start..end] {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
}

fn is_typescript_keyword(token: &[u8]) -> bool {
    matches!(
        token,
        b"abstract"
            | b"any"
            | b"as"
            | b"asserts"
            | b"async"
            | b"await"
            | b"bigint"
            | b"boolean"
            | b"break"
            | b"case"
            | b"catch"
            | b"class"
            | b"const"
            | b"constructor"
            | b"continue"
            | b"debugger"
            | b"declare"
            | b"default"
            | b"delete"
            | b"do"
            | b"else"
            | b"enum"
            | b"export"
            | b"extends"
            | b"false"
            | b"finally"
            | b"for"
            | b"from"
            | b"function"
            | b"get"
            | b"if"
            | b"implements"
            | b"import"
            | b"in"
            | b"infer"
            | b"instanceof"
            | b"interface"
            | b"is"
            | b"keyof"
            | b"let"
            | b"module"
            | b"namespace"
            | b"never"
            | b"new"
            | b"null"
            | b"number"
            | b"object"
            | b"of"
            | b"out"
            | b"override"
            | b"private"
            | b"protected"
            | b"public"
            | b"readonly"
            | b"require"
            | b"return"
            | b"satisfies"
            | b"set"
            | b"static"
            | b"string"
            | b"super"
            | b"switch"
            | b"symbol"
            | b"this"
            | b"throw"
            | b"true"
            | b"try"
            | b"type"
            | b"typeof"
            | b"undefined"
            | b"unique"
            | b"unknown"
            | b"using"
            | b"var"
            | b"void"
            | b"while"
            | b"with"
            | b"yield"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        csharp_recent_syntax_parser_view, go_new_value_parser_view,
        typescript_recent_syntax_parser_view,
    };

    #[test]
    fn go_masks_new_operands_except_type_literals() {
        let source = "var a = new(\"value\")\nvar b = new(int)\nvar c = new(factory())\nvar d = new(left + right)\nvar e = new([]byte)\nvar f = new(map[string]int)\n";
        let view = go_new_value_parser_view(source).unwrap();
        assert_eq!(view.len(), source.len());
        assert!(view.contains("ptr(\"value\")"));
        assert!(view.contains("ptr(int)"));
        assert!(view.contains("ptr(factory())"));
        assert!(view.contains("ptr(left + right)"));
        assert!(view.contains("new([]byte)"));
        assert!(view.contains("new(map[string]int)"));
    }

    #[test]
    fn typescript_masks_variance_and_keyword_tuple_labels() {
        let source =
            "interface Box<in out T, out U> {}\ntype Row = [symbol: Symbol, value?: string];\n";
        let view = typescript_recent_syntax_parser_view(source).unwrap();
        assert_eq!(view.len(), source.len());
        assert!(view.contains("Box<       T,     U>"));
        assert!(view.contains("[x_____: Symbol, value?: string]"));
    }

    #[test]
    fn csharp_masks_allows_ref_struct_as_an_ordinary_constraint() {
        let source = "void M<T>() where T : IFoo, allows ref struct { value = ref other; return ref value; var x = *(int*)&value; var y = *(T*)pointer; }\nextension<T>(IEnumerable<T> values) where T : class { public bool Any => true; }";
        let view = csharp_recent_syntax_parser_view(source).unwrap();
        assert_eq!(view.len(), source.len());
        assert!(view.contains("IFoo, PiraConstraint___"));
        assert!(view.contains("value =     other"));
        assert!(view.contains("return     value"));
        assert!(view.contains("var x = default"));
        assert!(view.contains("var y = default"));
        assert!(view.contains("class E"));
        assert!(!view.contains("extension<T>"));
    }

    #[test]
    fn recovery_scanners_ignore_comments_and_strings() {
        assert!(go_new_value_parser_view("// new(\"x\")\nvar s = `new(1)`\n").is_none());
        assert!(
            typescript_recent_syntax_parser_view("// <in T>\nconst s = '[symbol: X]';\n").is_none()
        );
        assert!(
            csharp_recent_syntax_parser_view(
                "// allows ref struct\n\"allows ref struct\"\n@\"value = ref other\"\n\"\"\"return ref value\"\"\"\n"
            )
            .is_none()
        );
    }
}
