/// Produces a byte-for-byte aligned parser view with common C-family macro
/// annotations neutralized. The original source remains authoritative for all
/// displayed text and hashes.
pub fn normalized_parser_view(source: &str) -> Option<String> {
    let original = source.as_bytes();
    let mut output = original.to_vec();
    let mut changed = mask_define_directives(original, &mut output);
    let mut namespace_boundaries = Vec::<(usize, usize, NamespaceBoundary)>::new();
    let mut namespace_depth = 0usize;
    let mut namespace_boundaries_valid = true;
    let mut index = 0;

    while index < original.len() {
        if let Some(next) = skip_non_code(original, index) {
            index = next;
            continue;
        }
        if !is_identifier_start(original[index]) {
            index += 1;
            continue;
        }

        let start = index;
        index += 1;
        while index < original.len() && is_identifier_continue(original[index]) {
            index += 1;
        }
        let name = &original[start..index];
        let previous = previous_non_whitespace(original, start);
        let next = next_non_whitespace(original, index);

        if is_upper_macro_name(name)
            && contains_ascii_case_insensitive(name, b"namespace")
            && line_prefix_is_whitespace(original, start)
            && line_suffix_is_whitespace_or_comment(original, index)
        {
            if contains_ascii_case_insensitive(name, b"begin") {
                namespace_depth += 1;
                namespace_boundaries.push((start, index, NamespaceBoundary::Begin));
            } else if contains_ascii_case_insensitive(name, b"end") {
                if let Some(next_depth) = namespace_depth.checked_sub(1) {
                    namespace_depth = next_depth;
                    namespace_boundaries.push((start, index, NamespaceBoundary::End));
                } else {
                    namespace_boundaries_valid = false;
                }
            }
        }

        let lambda_annotation = is_annotation_name(name)
            && previous.is_some_and(|position| original[position] == b']')
            && next.is_some_and(|position| matches!(original[position], b'(' | b'{'));
        let restrict_qualifier =
            is_annotation_name(name) && contains_ascii_case_insensitive(name, b"restrict");
        if lambda_annotation || restrict_qualifier {
            mask_non_newlines(&mut output, start, index);
            changed = true;
        }

        if name == b"typename"
            && next.is_some_and(|position| qualified_name_starts_at(original, position))
        {
            mask_non_newlines(&mut output, start, index);
            changed = true;
        }

        let Some(open) = next.filter(|position| original[*position] == b'(') else {
            continue;
        };
        let Some(close) = matching_parenthesis(original, open) else {
            continue;
        };

        if is_function_like_annotation(name) {
            mask_non_newlines(&mut output, start, close + 1);
            changed = true;
            index = close + 1;
            continue;
        }

        let after_close = next_non_whitespace(original, close + 1);
        if is_upper_macro_name(name)
            && after_close.is_some_and(|position| original[position] == b'(')
            && !has_top_level_comma(original, open, close)
        {
            mask_non_newlines(&mut output, start, open + 1);
            mask_non_newlines(&mut output, close, close + 1);
            changed = true;
            index = close + 1;
            continue;
        }
        if is_upper_macro_name(name)
            && after_close
                .is_some_and(|position| original.get(position..position + 2) == Some(b"::"))
            && !has_top_level_comma(original, open, close)
        {
            mask_non_newlines(&mut output, start, open + 1);
            mask_non_newlines(&mut output, close, close + 1);
            changed = true;
            index = close + 1;
            continue;
        }

        if is_upper_macro_name(name)
            && line_prefix_is_whitespace(original, start)
            && line_suffix_is_whitespace_or_comment(original, close + 1)
        {
            mask_non_newlines(&mut output, start, close + 1);
            changed = true;
            index = close + 1;
            continue;
        }

        if contains_ascii_case_insensitive(name, b"stringify") {
            mask_non_newlines(&mut output, open + 1, close);
            changed = true;
            index = close + 1;
        }
    }

    if namespace_boundaries_valid
        && namespace_depth == 0
        && !namespace_boundaries.is_empty()
        && namespace_boundaries.iter().all(|(start, end, boundary)| {
            !matches!(boundary, NamespaceBoundary::Begin) || end - start >= b"namespace {".len()
        })
    {
        for (start, end, boundary) in namespace_boundaries {
            mask_non_newlines(&mut output, start, end);
            let replacement = match boundary {
                NamespaceBoundary::Begin => b"namespace {".as_slice(),
                NamespaceBoundary::End => b"}",
            };
            output[start..start + replacement.len()].copy_from_slice(replacement);
        }
        changed = true;
    }

    changed |= mask_sfinae_parameters(original, &mut output);
    changed |= normalize_empty_default_braces(original, &mut output);

    changed.then(|| {
        // Only ASCII code bytes are replaced, so UTF-8 validity and every byte
        // and line offset remain unchanged.
        String::from_utf8(output).expect("length-preserving ASCII masking preserves UTF-8")
    })
}

#[derive(Clone, Copy)]
enum NamespaceBoundary {
    Begin,
    End,
}

/// Adds a conservative second recovery layer for syntax that parsers commonly
/// cannot see through: conditional preprocessor structure and declaration
/// annotation macros. It is only useful when the caller parses and compares the
/// result with the less-normalized view.
pub fn aggressive_parser_view(source: &str, base: &str) -> Option<String> {
    let mut output = base.as_bytes().to_vec();
    let changed = mask_directives(source.as_bytes(), &mut output, DirectiveMode::NonInclude)
        | mask_declaration_annotations(source.as_bytes(), &mut output);
    changed.then(|| {
        String::from_utf8(output).expect("length-preserving ASCII masking preserves UTF-8")
    })
}

/// Focused variants are only needed when combining the two aggressive passes
/// does not improve the base parse. Keeping them lazy avoids routine extra
/// parser invocations.
pub fn focused_parser_views(source: &str, base: &str) -> Vec<String> {
    let original = source.as_bytes();
    let mut views = Vec::with_capacity(4);

    for starting_view in [base.as_bytes(), original] {
        let mut preprocessor = starting_view.to_vec();
        if mask_directives(original, &mut preprocessor, DirectiveMode::NonInclude) {
            push_unique_view(&mut views, preprocessor);
        }

        let mut annotations = starting_view.to_vec();
        if mask_declaration_annotations(original, &mut annotations) {
            push_unique_view(&mut views, annotations);
        }
    }

    let mut type_syntax = original.to_vec();
    if mask_sfinae_parameters(original, &mut type_syntax)
        | normalize_empty_default_braces(original, &mut type_syntax)
    {
        push_unique_view(&mut views, type_syntax);
    }
    views
}

/// Produces two configuration-shaped views for balanced conditional
/// preprocessor blocks: one keeps each first arm and one keeps each final arm.
/// Includes remain visible in every arm so dependency discovery stays complete.
pub fn conditional_branch_parser_views(source: &str, base: &str) -> Vec<String> {
    conditional_branch_views(source, base, true)
}

/// Language-neutral conditional-compilation views.  C# directives have the
/// same balanced arm structure, but C/C++ declaration-macro masking must not be
/// applied to their source.
pub fn conditional_branch_parser_views_plain(source: &str, base: &str) -> Vec<String> {
    conditional_branch_views(source, base, false)
}

fn conditional_branch_views(source: &str, base: &str, mask_annotations: bool) -> Vec<String> {
    let Some((directives, arm_counts)) = conditional_directives(source.as_bytes()) else {
        return Vec::new();
    };
    if arm_counts.is_empty() {
        return Vec::new();
    }

    let mut starting_view = base.as_bytes().to_vec();
    if mask_annotations {
        mask_declaration_annotations(source.as_bytes(), &mut starting_view);
    }
    let mut views = Vec::with_capacity(2);
    for keep_final_arm in [false, true] {
        let selected_arms = arm_counts
            .iter()
            .map(|count| if keep_final_arm { count - 1 } else { 0 })
            .collect::<Vec<_>>();
        let mut output = starting_view.clone();
        let mut stack = Vec::<(usize, usize)>::new();
        let mut cursor = 0usize;

        for directive in &directives {
            if !conditional_region_is_active(&stack, &selected_arms) {
                mask_non_newlines(&mut output, cursor, directive.start);
            }
            match directive.kind {
                ConditionalDirectiveKind::Start(group) => stack.push((group, 0)),
                ConditionalDirectiveKind::Next(group, arm) => {
                    let Some(current) = stack.last_mut() else {
                        return Vec::new();
                    };
                    if current.0 != group {
                        return Vec::new();
                    }
                    current.1 = arm;
                }
                ConditionalDirectiveKind::End(group) => {
                    if stack.pop().is_none_or(|current| current.0 != group) {
                        return Vec::new();
                    }
                }
                ConditionalDirectiveKind::Include => {}
                ConditionalDirectiveKind::Other => {}
            }

            let preserve_include = matches!(directive.kind, ConditionalDirectiveKind::Include);
            // Inactive includes are also preserved: dependency extraction
            // should describe the source, not one guessed configuration.
            if !preserve_include {
                mask_non_newlines(&mut output, directive.start, directive.end);
            }
            cursor = directive.end;
        }
        if !conditional_region_is_active(&stack, &selected_arms) {
            let end = output.len();
            mask_non_newlines(&mut output, cursor, end);
        }
        push_unique_view(&mut views, output);
    }
    views
}

/// Masks statement-like uppercase macro calls that contain an inline lambda.
/// Such wrappers commonly perform type dispatch; their lambda bodies contain
/// implementation details rather than separately navigable declarations.
pub fn macro_lambda_statement_parser_view(source: &str, base: &str) -> Option<String> {
    let original = source.as_bytes();
    let mut output = base.as_bytes().to_vec();
    if !mask_macro_lambda_statements(original, &mut output) {
        return None;
    }
    mask_directives(original, &mut output, DirectiveMode::NonInclude);
    mask_declaration_annotations(original, &mut output);
    Some(String::from_utf8(output).expect("length-preserving ASCII masking preserves UTF-8"))
}

fn mask_macro_lambda_statements(source: &[u8], output: &mut [u8]) -> bool {
    let mut changed = false;
    let mut index = 0usize;
    while index < source.len() {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        if !is_identifier_start(source[index]) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < source.len() && is_identifier_continue(source[index]) {
            index += 1;
        }
        let name = &source[start..index];
        if !is_upper_macro_name(name) || !line_prefix_is_whitespace(source, start) {
            continue;
        }
        let Some(open) =
            next_non_whitespace(source, index).filter(|position| source[*position] == b'(')
        else {
            continue;
        };
        let Some(close) = matching_parenthesis(source, open) else {
            continue;
        };
        let Some(semicolon) =
            next_non_whitespace(source, close + 1).filter(|position| source[*position] == b';')
        else {
            continue;
        };
        if !line_suffix_is_whitespace_or_comment(source, semicolon + 1)
            || !contains_lambda_expression(source, open + 1, close)
        {
            continue;
        }
        mask_non_newlines(output, start, close + 1);
        changed = true;
        index = semicolon + 1;
    }
    changed
}

fn contains_lambda_expression(source: &[u8], start: usize, end: usize) -> bool {
    let mut index = start;
    while index < end {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        if source[index] != b'[' {
            index += 1;
            continue;
        }
        let Some(capture_end) = matching_square(source, index, end) else {
            return false;
        };
        let mut after = next_non_whitespace(source, capture_end + 1).unwrap_or(end);
        if after < end && source[after] == b'(' {
            let Some(parameters_end) = matching_parenthesis(source, after) else {
                return false;
            };
            after = next_non_whitespace(source, parameters_end + 1).unwrap_or(end);
        }
        if after < end && source[after] == b'{' {
            return true;
        }
        index = capture_end + 1;
    }
    false
}

fn matching_square(source: &[u8], open: usize, limit: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = open;
    while index < limit {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        match source[index] {
            b'[' => depth += 1,
            b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

#[derive(Clone, Copy)]
struct ConditionalDirective {
    start: usize,
    end: usize,
    kind: ConditionalDirectiveKind,
}

#[derive(Clone, Copy)]
enum ConditionalDirectiveKind {
    Start(usize),
    Next(usize, usize),
    End(usize),
    Include,
    Other,
}

fn conditional_directives(source: &[u8]) -> Option<(Vec<ConditionalDirective>, Vec<usize>)> {
    let mut directives = Vec::new();
    let mut arm_counts = Vec::new();
    let mut stack = Vec::<(usize, usize)>::new();
    let mut index = 0usize;
    while index < source.len() {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        if source[index] != b'#' || !line_prefix_is_whitespace(source, index) {
            index += 1;
            continue;
        }

        let line_start = source[..index]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |position| position + 1);
        let line_end = physical_line_end(source, index);
        let logical_end = directive_logical_end(source, line_end);
        let mut name_start = index + 1;
        while name_start < line_end && source[name_start].is_ascii_whitespace() {
            name_start += 1;
        }
        let mut name_end = name_start;
        while name_end < line_end && is_identifier_continue(source[name_end]) {
            name_end += 1;
        }
        let name = &source[name_start..name_end];
        let kind = match name {
            b"if" | b"ifdef" | b"ifndef" => {
                let group = arm_counts.len();
                arm_counts.push(1);
                stack.push((group, 0));
                ConditionalDirectiveKind::Start(group)
            }
            b"elif" | b"elifdef" | b"elifndef" | b"else" => {
                let current = stack.last_mut()?;
                current.1 += 1;
                arm_counts[current.0] = current.1 + 1;
                ConditionalDirectiveKind::Next(current.0, current.1)
            }
            b"endif" => {
                let (group, _) = stack.pop()?;
                ConditionalDirectiveKind::End(group)
            }
            b"include" | b"include_next" => ConditionalDirectiveKind::Include,
            _ => ConditionalDirectiveKind::Other,
        };
        directives.push(ConditionalDirective {
            start: line_start,
            end: logical_end,
            kind,
        });
        index = logical_end.saturating_add(1);
    }
    stack.is_empty().then_some((directives, arm_counts))
}

fn conditional_region_is_active(stack: &[(usize, usize)], selected_arms: &[usize]) -> bool {
    stack
        .iter()
        .all(|(group, arm)| selected_arms.get(*group) == Some(arm))
}

fn push_unique_view(views: &mut Vec<String>, bytes: Vec<u8>) {
    if !views.iter().any(|view| view.as_bytes() == bytes) {
        views.push(
            String::from_utf8(bytes).expect("length-preserving ASCII masking preserves UTF-8"),
        );
    }
}

#[derive(Clone, Copy)]
enum DirectiveMode {
    Defines,
    NonInclude,
}

fn mask_define_directives(source: &[u8], output: &mut [u8]) -> bool {
    mask_directives(source, output, DirectiveMode::Defines)
}

fn mask_directives(source: &[u8], output: &mut [u8], mode: DirectiveMode) -> bool {
    let mut changed = false;
    let mut index = 0usize;
    while index < source.len() {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        if source[index] != b'#' || !line_prefix_is_whitespace(source, index) {
            index += 1;
            continue;
        }

        let line_start = source[..index]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |position| position + 1);
        let line_end = physical_line_end(source, index);
        let mut name_start = index + 1;
        while name_start < line_end && source[name_start].is_ascii_whitespace() {
            name_start += 1;
        }
        let mut name_end = name_start;
        while name_end < line_end && is_identifier_continue(source[name_end]) {
            name_end += 1;
        }
        let name = &source[name_start..name_end];
        let should_mask = match mode {
            DirectiveMode::Defines => name == b"define",
            DirectiveMode::NonInclude => !matches!(name, b"include" | b"include_next"),
        };
        let logical_end = directive_logical_end(source, line_end);
        if should_mask {
            mask_non_newlines(output, line_start, logical_end);
            changed = true;
        }
        index = logical_end.saturating_add(1);
    }
    changed
}

fn physical_line_end(source: &[u8], start: usize) -> usize {
    source[start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(source.len(), |offset| start + offset)
}

fn directive_logical_end(source: &[u8], mut line_end: usize) -> usize {
    loop {
        let line_start = source[..line_end]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |position| position + 1);
        let content_end = source[line_start..line_end]
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .map(|offset| line_start + offset);
        if content_end.is_none_or(|position| source[position] != b'\\') || line_end == source.len()
        {
            return line_end;
        }
        line_end = physical_line_end(source, line_end + 1);
    }
}

fn mask_declaration_annotations(source: &[u8], output: &mut [u8]) -> bool {
    let mut changed = false;
    let mut index = 0usize;
    while index < source.len() {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        if source[index] == b'#' && line_prefix_is_whitespace(source, index) {
            index =
                directive_logical_end(source, physical_line_end(source, index)).saturating_add(1);
            continue;
        }
        if !is_identifier_start(source[index]) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < source.len() && is_identifier_continue(source[index]) {
            index += 1;
        }
        let name = &source[start..index];
        if is_declaration_annotation(name)
            && next_non_whitespace(source, index).is_none_or(|position| source[position] != b'(')
        {
            mask_non_newlines(output, start, index);
            changed = true;
        }
    }
    changed
}

fn normalize_empty_default_braces(source: &[u8], output: &mut [u8]) -> bool {
    let mut changed = false;
    let mut index = 0usize;
    while index < source.len() {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        if source[index] != b'=' {
            index += 1;
            continue;
        }
        let Some(open) =
            next_non_whitespace(source, index + 1).filter(|position| source[*position] == b'{')
        else {
            index += 1;
            continue;
        };
        let Some(close) =
            next_non_whitespace(source, open + 1).filter(|position| source[*position] == b'}')
        else {
            index += 1;
            continue;
        };
        output[open] = b'0';
        output[close] = b' ';
        changed = true;
        index = close + 1;
    }
    changed
}

fn mask_sfinae_parameters(source: &[u8], output: &mut [u8]) -> bool {
    let mut changed = false;
    let mut index = 0usize;
    while index < source.len() {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        if !is_identifier_start(source[index]) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < source.len() && is_identifier_continue(source[index]) {
            index += 1;
        }
        let token = &source[start..index];
        if token != b"typename" && token != b"std" {
            continue;
        }
        let Some((end, has_name)) = sfinae_parameter_span(source, token, index) else {
            continue;
        };
        mask_non_newlines(output, start, end);
        let replacement = if has_name {
            b"int".as_slice()
        } else {
            b"int p"
        };
        output[start..start + replacement.len()].copy_from_slice(replacement);
        changed = true;
        index = end;
    }
    changed
}

fn qualified_name_starts_at(source: &[u8], start: usize) -> bool {
    if start >= source.len() || !is_identifier_start(source[start]) {
        return false;
    }
    let mut end = start + 1;
    while end < source.len() && is_identifier_continue(source[end]) {
        end += 1;
    }
    next_non_whitespace(source, end)
        .is_some_and(|position| source.get(position..position + 2) == Some(b"::"))
}

fn sfinae_parameter_span(
    source: &[u8],
    first_token: &[u8],
    first_end: usize,
) -> Option<(usize, bool)> {
    let std_start = if first_token == b"typename" {
        next_non_whitespace(source, first_end)?
    } else {
        first_end - first_token.len()
    };
    let std_end = identifier_end(source, std_start)?;
    if &source[std_start..std_end] != b"std" {
        return None;
    }
    let first_colons = next_non_whitespace(source, std_end)?;
    if source.get(first_colons..first_colons + 2) != Some(b"::") {
        return None;
    }
    let enable_start = next_non_whitespace(source, first_colons + 2)?;
    let enable_end = identifier_end(source, enable_start)?;
    if &source[enable_start..enable_end] != b"enable_if_t" {
        return None;
    }
    let open = next_non_whitespace(source, enable_end)?;
    if source[open] != b'<' {
        return None;
    }
    let close = matching_angle(source, open)?;
    let pointer = next_non_whitespace(source, close + 1)?;
    if source[pointer] != b'*' {
        return None;
    }
    let after_pointer = next_non_whitespace(source, pointer + 1)?;
    let (has_name, after_parameter) = if source[after_pointer] == b'=' {
        (false, after_pointer)
    } else {
        let name_end = identifier_end(source, after_pointer)?;
        (true, next_non_whitespace(source, name_end)?)
    };
    (source[after_parameter] == b'=').then_some((pointer + 1, has_name))
}

fn identifier_end(source: &[u8], start: usize) -> Option<usize> {
    if start >= source.len() || !is_identifier_start(source[start]) {
        return None;
    }
    let mut end = start + 1;
    while end < source.len() && is_identifier_continue(source[end]) {
        end += 1;
    }
    Some(end)
}

fn matching_angle(source: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = open;
    while index < source.len() {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        match source[index] {
            b'<' => depth += 1,
            b'>' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn is_declaration_annotation(name: &[u8]) -> bool {
    const HINTS: [&[u8]; 13] = [
        b"device",
        b"host",
        b"inline",
        b"export",
        b"api",
        b"restrict",
        b"launch",
        b"global",
        b"kernel",
        b"call",
        b"attribute",
        b"visibility",
        b"deprecated",
    ];
    name.eq_ignore_ascii_case(b"inline")
        || (is_annotation_name(name)
            && HINTS
                .iter()
                .any(|hint| contains_ascii_case_insensitive(name, hint)))
}

fn is_function_like_annotation(name: &[u8]) -> bool {
    const HINTS: [&[u8]; 5] = [
        b"attribute",
        b"align",
        b"launch_bounds",
        b"declspec",
        b"bounds",
    ];
    is_annotation_name(name)
        && HINTS
            .iter()
            .any(|hint| contains_ascii_case_insensitive(name, hint))
}

fn skip_non_code(source: &[u8], index: usize) -> Option<usize> {
    if source[index] == b'/' && source.get(index + 1) == Some(&b'/') {
        return Some(
            source[index + 2..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(source.len(), |offset| index + 2 + offset),
        );
    }
    if source[index] == b'/' && source.get(index + 1) == Some(&b'*') {
        return Some(
            source[index + 2..]
                .windows(2)
                .position(|window| window == b"*/")
                .map_or(source.len(), |offset| index + 2 + offset + 2),
        );
    }
    if matches!(source[index], b'\'' | b'"') {
        if source[index] == b'"' {
            let quote_run = source[index..]
                .iter()
                .take_while(|byte| **byte == b'"')
                .count();
            if quote_run >= 3 {
                return Some(skip_raw_quotes(source, index, quote_run));
            }
            if source.get(index.wrapping_sub(1)) == Some(&b'@')
                || (index >= 2 && matches!(&source[index - 2..index], b"@$" | b"$@"))
            {
                return Some(skip_verbatim_quoted(source, index));
            }
        }
        return Some(skip_quoted(source, index, source[index]));
    }
    if source[index] == b'R' && source.get(index + 1) == Some(&b'"') {
        return raw_string_end(source, index).or(Some(index + 1));
    }
    None
}

fn skip_raw_quotes(source: &[u8], start: usize, quote_run: usize) -> usize {
    let mut index = start + quote_run;
    while index < source.len() {
        if source[index] == b'"'
            && source[index..]
                .iter()
                .take_while(|byte| **byte == b'"')
                .count()
                >= quote_run
        {
            return index + quote_run;
        }
        index += 1;
    }
    source.len()
}

fn skip_verbatim_quoted(source: &[u8], start: usize) -> usize {
    let mut index = start + 1;
    while index < source.len() {
        if source[index] != b'"' {
            index += 1;
        } else if source.get(index + 1) == Some(&b'"') {
            index += 2;
        } else {
            return index + 1;
        }
    }
    source.len()
}

fn skip_quoted(source: &[u8], start: usize, quote: u8) -> usize {
    let mut index = start + 1;
    while index < source.len() {
        if source[index] == b'\\' {
            index = (index + 2).min(source.len());
        } else if source[index] == quote {
            return index + 1;
        } else {
            index += 1;
        }
    }
    source.len()
}

fn raw_string_end(source: &[u8], start: usize) -> Option<usize> {
    let delimiter_start = start + 2;
    let open = source[delimiter_start..]
        .iter()
        .position(|byte| *byte == b'(')
        .map(|offset| delimiter_start + offset)?;
    if open - delimiter_start > 16 {
        return None;
    }
    let delimiter = &source[delimiter_start..open];
    let mut index = open + 1;
    while index < source.len() {
        if source[index] == b')'
            && source.get(index + 1..index + 1 + delimiter.len()) == Some(delimiter)
            && source.get(index + 1 + delimiter.len()) == Some(&b'"')
        {
            return Some(index + delimiter.len() + 2);
        }
        index += 1;
    }
    Some(source.len())
}

fn matching_parenthesis(source: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = open;
    while index < source.len() {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        match source[index] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn has_top_level_comma(source: &[u8], open: usize, close: usize) -> bool {
    let mut depth = 0usize;
    let mut index = open + 1;
    while index < close {
        if let Some(next) = skip_non_code(source, index) {
            index = next;
            continue;
        }
        match source[index] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => return true,
            _ => {}
        }
        index += 1;
    }
    false
}

fn previous_non_whitespace(source: &[u8], start: usize) -> Option<usize> {
    (0..start)
        .rev()
        .find(|position| !source[*position].is_ascii_whitespace())
}

fn next_non_whitespace(source: &[u8], start: usize) -> Option<usize> {
    (start..source.len()).find(|position| !source[*position].is_ascii_whitespace())
}

fn line_prefix_is_whitespace(source: &[u8], start: usize) -> bool {
    let line_start = source[..start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    source[line_start..start]
        .iter()
        .all(|byte| byte.is_ascii_whitespace())
}

fn line_suffix_is_whitespace_or_comment(source: &[u8], start: usize) -> bool {
    let line_end = source[start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(source.len(), |offset| start + offset);
    let suffix = &source[start..line_end];
    let trimmed = suffix
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(&[][..], |offset| &suffix[offset..]);
    trimmed.is_empty() || trimmed.starts_with(b"//") || trimmed.starts_with(b"/*")
}

fn mask_non_newlines(output: &mut [u8], start: usize, end: usize) {
    for byte in &mut output[start..end] {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_upper_macro_name(name: &[u8]) -> bool {
    name.len() >= 3
        && name.iter().any(u8::is_ascii_uppercase)
        && name
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn is_annotation_name(name: &[u8]) -> bool {
    is_upper_macro_name(name)
        || (name.len() > 4 && name.starts_with(b"__") && name.ends_with(b"__"))
}

fn contains_ascii_case_insensitive(value: &[u8], needle: &[u8]) -> bool {
    value.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        aggressive_parser_view, conditional_branch_parser_views,
        conditional_branch_parser_views_plain, focused_parser_views,
        macro_lambda_statement_parser_view, normalized_parser_view,
    };

    #[test]
    fn masks_annotations_without_changing_offsets() {
        let source = "C10_BOUNDS(256)\n__global__ void run() {}\n\
                      auto f = [] GPU_LAMBDA(int x) { return x; };\n\
                      int* RESTRICT value;\n";
        let normalized = normalized_parser_view(source).expect("changed");
        assert_eq!(normalized.len(), source.len());
        assert_eq!(
            normalized.matches('\n').count(),
            source.matches('\n').count()
        );
        assert!(!normalized.contains("C10_BOUNDS"));
        assert!(!normalized.contains("GPU_LAMBDA"));
        assert!(!normalized.contains("RESTRICT"));
        assert!(normalized.contains("__global__ void run"));
    }

    #[test]
    fn leaves_comments_strings_and_regular_calls_unchanged() {
        let source = "// GPU_LAMBDA(x)\nconst char* s = \"RESTRICT\";\nregular_call(value);\n";
        assert!(normalized_parser_view(source).is_none());
    }

    #[test]
    fn masks_stringified_code_but_preserves_call_shape() {
        let source = "auto text = code_stringify(template <class T> T f(T x) { return x; });\n";
        let normalized = normalized_parser_view(source).expect("changed");
        assert_eq!(normalized.len(), source.len());
        assert!(normalized.contains("code_stringify("));
        assert!(!normalized.contains("template"));
    }

    #[test]
    fn masks_defines_and_unwraps_qualified_macro_calls() {
        let source = "#define GENERATED(x) \\\n  void x() {}\nusing Type = PLATFORM_NS(project::detail)::Type;\n";
        let normalized = normalized_parser_view(source).expect("changed");
        assert_eq!(normalized.len(), source.len());
        assert!(!normalized.contains("#define"));
        assert!(normalized.contains("project::detail ::Type"));
    }

    #[test]
    fn normalizes_macro_generated_declarations_and_defaults() {
        let source = "TORCH_IMPL_FUNC(run)\n\
(int value) {}\n\
struct __align__(16) Packet {};\n\
template <typename std::enable_if_t<true>* = nullptr> void enabled();\n\
void defaults(const Packet& packet = {});\n";
        let normalized = normalized_parser_view(source).expect("changed");
        assert_eq!(normalized.len(), source.len());
        assert!(!normalized.contains("TORCH_IMPL_FUNC"));
        assert!(normalized.contains("run \n(int value)"));
        assert!(!normalized.contains("__align__"));
        assert!(normalized.contains("struct               Packet {}"));
        assert!(!normalized.contains("typename std::enable_if_t"));
        assert!(normalized.contains("template <int p"));
        assert!(normalized.contains("packet = 0 "));
    }

    #[test]
    fn normalizes_named_and_unnamed_sfinae_value_parameters() {
        let source = "template <typename T, std::enable_if_t<true>* = nullptr> void first();\n\
                      template <typename T, typename std::enable_if_t<true>* Guard = nullptr> void second();\n";
        let normalized = normalized_parser_view(source).expect("changed");
        assert_eq!(normalized.len(), source.len());
        assert!(normalized.contains("template <typename T, int p"));
        assert!(normalized.contains("template <typename T, int"));
        assert!(normalized.contains("Guard = nullptr"));
        assert!(!normalized.contains("enable_if_t"));
    }

    #[test]
    fn aggressive_view_preserves_dependencies_and_non_code_text() {
        let source = "#include <DEVICE_API.h>\n\
#define GENERATED 1\n\
#if FEATURE\n\
C10_DEVICE void run();\n\
#endif\n\
const char* text = \"#endif C10_DEVICE\";\n\
/*\n#if COMMENT\nC10_DEVICE\n*/\n\
regular_call(value);\n";
        let base = normalized_parser_view(source).expect("base changed");
        let aggressive = aggressive_parser_view(source, &base).expect("aggressive changed");
        assert_eq!(aggressive.len(), source.len());
        assert!(aggressive.contains("#include <DEVICE_API.h>"));
        assert!(!aggressive.contains("#if FEATURE"));
        assert!(!aggressive.contains("C10_DEVICE void run"));
        assert!(aggressive.contains("\"#endif C10_DEVICE\""));
        assert!(aggressive.contains("#if COMMENT"));
        assert!(aggressive.contains("regular_call(value)"));

        let focused = focused_parser_views(source, &base);
        assert_eq!(focused.len(), 3);
        assert!(focused.iter().all(|view| view.len() == source.len()));
    }

    #[test]
    fn conditional_views_keep_each_branch_and_all_includes() {
        let source = "void run(\n\
#if FEATURE\n\
  int first\n\
  #include \"first.h\"\n\
#else\n\
  float second\n\
  #include \"second.h\"\n\
#endif\n\
) {}\n";
        let views = conditional_branch_parser_views(source, source);
        assert_eq!(views.len(), 2);
        assert!(views.iter().all(|view| view.len() == source.len()));
        assert!(
            views
                .iter()
                .all(|view| view.contains("#include \"first.h\""))
        );
        assert!(
            views
                .iter()
                .all(|view| view.contains("#include \"second.h\""))
        );
        assert!(views[0].contains("int first"));
        assert!(!views[0].contains("float second"));
        assert!(!views[1].contains("int first"));
        assert!(views[1].contains("float second"));
        assert!(views.iter().all(|view| !view.contains("#if FEATURE")));
    }

    #[test]
    fn conditional_views_reject_unbalanced_input() {
        let source = "#if FEATURE\nvoid run();\n";
        assert!(conditional_branch_parser_views(source, source).is_empty());
    }

    #[test]
    fn plain_conditional_views_ignore_directive_text_in_raw_strings() {
        let source = "var text = \"\"\"\n#if NOT_A_DIRECTIVE\n#endif\n\"\"\";\n#if REAL\nvoid First() {}\n#else\nvoid Second() {}\n#endif\n";
        let views = conditional_branch_parser_views_plain(source, source);
        assert_eq!(views.len(), 2);
        assert!(
            views
                .iter()
                .all(|view| view.contains("#if NOT_A_DIRECTIVE"))
        );
        assert!(views[0].contains("void First"));
        assert!(views[1].contains("void Second"));
    }

    #[test]
    fn masks_only_statement_macros_with_inline_lambdas() {
        let source = "void run() {\n\
  TYPE_DISPATCH(value, [&] { nested_call(); });\n\
  REGULAR_MACRO(value);\n\
}\n";
        let view = macro_lambda_statement_parser_view(source, source).expect("lambda wrapper");
        assert_eq!(view.len(), source.len());
        assert!(!view.contains("TYPE_DISPATCH"));
        assert!(!view.contains("nested_call"));
        assert!(view.contains("REGULAR_MACRO(value);"));
    }

    #[test]
    fn normalizes_balanced_namespace_boundary_macros() {
        let source = "LIB_BEGIN_NAMESPACE\nvoid run();\nLIB_END_NAMESPACE\n";
        let normalized = normalized_parser_view(source).expect("namespace boundaries");
        assert_eq!(normalized.len(), source.len());
        assert!(normalized.contains("namespace {"));
        assert!(!normalized.contains("LIB_BEGIN_NAMESPACE"));
        assert!(!normalized.contains("LIB_END_NAMESPACE"));

        let unbalanced = "LIB_BEGIN_NAMESPACE\nvoid run();\n";
        assert!(normalized_parser_view(unbalanced).is_none());
    }
}
