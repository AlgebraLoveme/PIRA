use std::io::Write;

use crate::command::{CommandResult, output_error};

const GLOBAL: &str = r#"USAGE
  pira_nav COMMAND [OPTIONS] [ARGUMENTS]
  pira_nav help COMMAND [COMMAND...]

COMMANDS
  map [PATH...]                     Repository shape, documents, and declarations.
  search -e PATTERN... [PATH...] Bounded search over ordinary unignored UTF-8 text.
  symbols QUERY [PATH...]        Declarations, keys, or headings.
  outline FILE...                Declarations or document paths without bodies.
  show TARGET...                 Exact items, position windows, or line ranges.
  imports | dependents | deps    Conservative syntax-level file relationships.
  definition | implementation | type-definition | references
                                  LSP symbol locations.
  callers | callees | supertypes | subtypes
                                  LSP call or type hierarchy.
  hover                           Bounded LSP hover text.
  query                           Source and semantic requests; shared LSP state.
  languages                       Formats and discovered language servers.

BACKENDS
  Bundled parsers handle clean source plus JSON/JSONC/YAML/TOML/Markdown. Dirty code uses a server
  discovered on PATH or selected by --lsp. --native requires a clean bundled parse. Semantic
  commands apply only to code and always require an LSP.

COMMON OPTIONS
  --language LANGUAGE       Override language inference for ambiguous paths.
  --native                  Require bundled structural parsing.
  --lsp [LANGUAGE=]PATH     Select an absolute language-server executable.
  --lsp-root DIR            Set the exact server workspace boundary.
  --                        End options; later arguments are positional where supported.

Read-only: never edits, builds, or executes repository code. Untrusted source/hover is framed; bounded
partial results identify omissions. Detailed syntax: `pira_nav COMMAND --help`; several topics:
`pira_nav help COMMAND...`."#;

const LSP_OPTIONS: &str = r#"  --language LANGUAGE
  --lsp [LANGUAGE=]ABSOLUTE_PATH
  --lsp-arg [LANGUAGE=]ARG
  --lsp-root DIR
  --lsp-init [LANGUAGE=]JSON_FILE
  --lsp-settings [LANGUAGE=]JSON_FILE"#;

const STRUCTURAL_OPTIONS: &str = r#"  --language LANGUAGE
  --native
  --lsp [LANGUAGE=]ABSOLUTE_PATH
  --lsp-arg [LANGUAGE=]ARG
  --lsp-root DIR
  --lsp-init [LANGUAGE=]JSON_FILE
  --lsp-settings [LANGUAGE=]JSON_FILE"#;

pub fn global(version: &str, output: &mut dyn Write) -> CommandResult {
    writeln!(
        output,
        "pira_nav {version} — read-only repository navigation\n"
    )
    .and_then(|()| writeln!(output, "{GLOBAL}"))
    .map_err(output_error)
}

pub fn command(name: &str, output: &mut dyn Write) -> CommandResult {
    let body = match name {
        "map" => format!(r#"pira_nav map — summarize repository or subsystem shape

USAGE
  pira_nav map [PATH...] [--limit N] [--max-depth N] [OPTIONS]

PATH defaults to `.`; up to 64 overlapping roots are deduplicated. Multiple-root rows are relative
to cwd. Output includes compact code/document counts, top directories, generic project
landmarks, and balanced representative declarations or top-level keys. At most 20 navigable rows are
shown by default. Ignored and hidden paths are outside directory scope; symlinked directories are not
followed. Recognizable fixture/corpus subtrees are counted but skipped by broad maps; map one directly
to inspect it structurally.

OPTIONS
{STRUCTURAL_OPTIONS}
  --limit N                 Maximum representative file rows (default 20).
  -g GLOB                   Filter files after ignore discovery; repeatable; ! excludes.
                            Cannot reinclude ignored files; filtering does not bound traversal.
  --max-depth N             Maximum filesystem traversal depth (0..256); 0 visits only PATH.

EXAMPLE
  pira_nav map src"#),
        "search" => r#"pira_nav search — bounded portable repository text search

USAGE
  pira_nav search PATTERN [PATH...] [OPTIONS]
  pira_nav search -e PATTERN [-e PATTERN]... [PATH...] [OPTIONS]

PATH defaults to `.`; up to 64 overlapping paths are deduplicated. Directories search unignored UTF-8
text; explicit files bypass ignore discovery but obey `--glob`. Missing peers mark output incomplete;
one missing target errors. Skipped files mark output incomplete; non-binary skip paths/reasons
are sampled with omitted counts.

OPTIONS
  -e PATTERN                Add a pattern (1..32 total).
  --language LANGUAGE       Restrict search to one supported language.
  --regex                   Use bounded Rust-regex syntax.
  -i                        Match without case distinctions.
  --word                    Require Unicode half-word boundaries.
  -g GLOB                   Restrict paths with a gitignore glob; repeatable; ! excludes.
  --files-with-matches      Print matching paths and query coverage only.
  --count                   Print exact matching-line counts per file/query.
  -B N                      Lines before each snippet match (maximum 1000).
  -A N                      Lines after each snippet match (maximum 1000).
  -C N                      Lines before and after each match (default 2, maximum 1000).
  --max-items N             Maximum shown lines/file rows (default 48).
  --limit N                 Snippet lines per pattern (default 8; 1..10000).
  --max-bytes N             Maximum rendered source-block bytes (default 8192).
  --owners                  Annotate snippet matches with enclosing clean declarations when available.

Combine -B and -A for asymmetric context; either conflicts with -C. One balanced scan ranks each
pattern independently with exact omission counts. --limit N without an explicit --max-items
sets the shared item cap to min(N * patterns, 10000). --limit is snippet-only. Shared item/byte caps
can reduce per-query output. Binding budgets are reported; context is reduced before source becomes location-only.
The byte cap excludes headers/diagnostics. Zero matches succeeds. Source is untrusted data;
lines over 512 bytes are clipped around the first selected match with byte-range metadata. A block that still
cannot fit after context reduction is metadata-only.

EXAMPLES
  pira_nav search Parser src
  pira_nav search -e Parser -e Compiler src
  pira_nav search 'impl\\s+Parser' --regex --word"#.into(),
        "symbols" => format!(r#"pira_nav symbols — ranked declaration and key discovery

USAGE
  pira_nav symbols QUERY [PATH...] [OPTIONS]
  pira_nav symbols -e QUERY [-e QUERY]... [PATH...] [OPTIONS]

PATH defaults to `.`; up to 64 overlapping paths are deduplicated. Searches code declaration names,
JSON/JSONC/YAML/TOML key paths, and Markdown headings, not arbitrary body text. With several paths,
missing peers mark output incomplete while valid peers are searched. Unique matches include bounded
exact source by default; use --locations-only to suppress it.
Default matching tries case-insensitive exact names/suffixes, then substrings only if no exact match.
--exact, --contains, and --regex are mutually exclusive.

OPTIONS
{STRUCTURAL_OPTIONS}
  -e QUERY                  Add a declaration/key query (1..32 total).
  --exact                   Case-insensitive full-name or qualified-suffix match; no fallback.
  --contains                Case-insensitive substring match.
  --regex                   Case-sensitive Rust regex (use (?i) to ignore case).
  --kind KIND               Restrict to a declaration/key/heading kind shown by outline.
  --limit N                 Rows per query (default 20).
  --selectors               Include freshness-checked selectors.
  --signatures              Include bounded signatures.
  --locations-only          Suppress source included for unique matches.

EXAMPLE
  pira_nav symbols Parser src tests"#),
        "outline" => format!(r#"pira_nav outline — declarations or document paths without bodies

USAGE
  pira_nav outline FILE... [OPTIONS]

OPTIONS
{STRUCTURAL_OPTIONS}
  --limit N                 Items across the invocation (default 64), shared among files.
  --depth N                 Maximum nested depth; 0 shows top-level items only.
  --match QUERY             Restrict declarations; repeatable.
  --signatures              Include bounded declaration signatures.
  --selectors               Include freshness-checked selectors.

Markdown headings are an indented tree of local canonical segments, bracket-quoted when needed.
Join ancestor segments with `::` to construct a target. Matching and `show FILE::ITEM` still use qualified heading paths.

EXAMPLE
  pira_nav outline src/parser.rs
  pira_nav outline .github/workflows/test.yaml
  pira_nav outline README.md"#),
        "show" => format!(r#"pira_nav show — bounded source retrieval

USAGE
  pira_nav show TARGET... [OPTIONS]

TARGET is FILE, FILE::QUALIFIED-NAME (or FILE::UNIQUE-SUFFIX), pira://selector,
FILE:LINE[:COLUMN], or FILE:START-END (signed bounds, -1 last file line). A bare file prints its full content. A position selects the
smallest enclosing named item; --window N selects parser-free surrounding lines. Exact files, line ranges,
and windows work for any readable UTF-8 text file without language inference. Source is printed exactly
apart from terminal-control escaping and is framed as untrusted data. --glance is a non-exact
orientation view: it prefixes every physical line with its line number and shows at most the first
160 source bytes, clipping only at UTF-8 boundaries and visibly marking omitted bytes.

Qualified names use `::` between hierarchy segments in every code and document format. Array indices
use [N]. Segments containing punctuation, whitespace, or `::` use JSON-style bracket quoting, for
example ["a.b"].
Shell-quote targets containing brackets or other metacharacters. Exact paths win over suffix
abbreviations; multiple matches error. Quote literal punctuation to distinguish it from hierarchy.

OPTIONS
{STRUCTURAL_OPTIONS}
  --window N                N lines before/after one FILE:LINE[:COLUMN]; replaces item selection.
  --range START:END         Inclusive lines within the preceding resolved TARGET (1-based).
                            Negative indices count from its end (-1 last); zero is invalid.
                            End past the boundary clips; invalid starts/reversed ranges error.
                            Applies to files, items, selectors, positions, and selected ranges.
  --glance                  Line-numbered preview; clips long lines, so not exact source.
  --max-items N             Batch target cap (default 20); not a line limit.
  --max-bytes N             Shared rendered-block byte budget; oversized blocks are omitted.

Single targets have no default byte cap. Batches default to 32768 bytes; explicit --max-bytes
also bounds a single target. Headers/diagnostics outside source blocks are not included.
--window requires one position target and cannot combine with --range.

EXAMPLES
  pira_nav show src/parser.rs::Parser::parse
  pira_nav show package.json::scripts::build
  pira_nav show workflow.yaml::jobs::test::steps[2]
  pira_nav show README.md::Install::Linux
  pira_nav show 'README.md::["Install > Linux"]'
  pira_nav show README.md
  pira_nav show README.md LICENSE
  pira_nav show README.md --range 1:20
  pira_nav show README.md --range -20:-1
  pira_nav show README.md LICENSE --range 1:20
  pira_nav show src/parser.rs:120-160
  pira_nav show generated.json:1-20 --glance"#),
        "imports" => r#"pira_nav imports — syntax-level file imports

USAGE
  pira_nav imports FILE... [--language LANGUAGE] [--max-items N]

Parses import/include syntax and conservatively resolves local paths inside the workspace. External,
unresolved, and blocked edges remain explicit. It never invokes a package manager or build system.
`--max-items` bounds rows per file (default 128, maximum 10000)."#.into(),
        "dependents" => r#"pira_nav dependents — reverse local import lookup

USAGE
  pira_nav dependents FILE [--root ROOT] [--language LANGUAGE] [--max-items N]

ROOT defaults to the current directory. FILE is resolved from the current directory, then ROOT when
the first path does not exist, and must lie within ROOT. Every eligible file is scanned; syntax failures
and unresolved relationships are accounted separately. Rows default to 128 and may be bounded up to
10000 with `--max-items`."#.into(),
        "deps" => r#"pira_nav deps — bounded local dependency traversal

USAGE
  pira_nav deps FILE [--root ROOT] [--direction imports|dependents|both] [--depth N]
      [--language LANGUAGE] [--max-items N]

FILE is resolved from the current directory, then ROOT when the first path does not exist, and must lie
within ROOT. Traverses only conservatively resolved local syntax edges. This is not a build graph.
Defaults: direction=both, depth=2, max-items=128. Depth is 0..256; max-items is at most 10000."#.into(),
        "definition" | "implementation" | "type-definition" | "references" | "callers"
        | "callees" | "supertypes" | "subtypes" => {
            let extra = if name == "references" {
                "\n  --include-declaration     Include declarations in reference results."
            } else {
                ""
            };
            format!(r#"pira_nav {name} — read-only LSP semantic navigation

USAGE
  pira_nav {name} TARGET... [OPTIONS]

TARGET is FILE:LINE:COLUMN, FILE::QUALIFIED-NAME, or pira://selector. Positions use one-based UTF-8 byte
coordinates. Exact qualified paths win over unique suffix abbreviations; selectors are freshness-checked. Results are bounded
and peer successes survive per-target failures.

OPTIONS
{LSP_OPTIONS}
  --limit N                 Maximum rows per target (maximum 10000).{extra}

EXAMPLE
  pira_nav {name} src/parser.rs::Parser::parse"#)
        }
        "hover" => format!(r#"pira_nav hover — bounded LSP hover text

USAGE
  pira_nav hover TARGET... [OPTIONS]

TARGET is FILE:LINE:COLUMN, FILE::QUALIFIED-NAME, or pira://selector. Hover is terminal-sanitized, byte
bounded, and framed as untrusted LSP data.

OPTIONS
{LSP_OPTIONS}
  --max-bytes N             Bytes per hover (default 16384, maximum 65536)."#),
        "query" => format!(r#"pira_nav query — ordered source and semantic requests

USAGE
  pira_nav query --OPERATION TARGET [--OPERATION TARGET]... [OPTIONS]

Operation options may repeat and execute in command-line order: --definition, --implementation,
--type-definition, --references, --callers, --callees, --supertypes, --subtypes, --hover, and --show.
Up to 32 requests; semantic requests share LSP state. This batches operations, not text/name searches.
For one operation on several targets, prefer its standalone command.
--show accepts all show targets without requiring an LSP; postfix --range START:END slices that
resolved content (inclusive, negative indices count from its end). Results/errors retain request
order. A failed request does not block others; partial success exits 0 with failed=N complete=0.
All failed requests exit nonzero. Options with no applicable operation error. Show blocks exceeding
the byte cap are omitted, not truncated.

OPTIONS
{LSP_OPTIONS}
  --limit N                 Row bound per semantic non-hover request (maximum 10000).
  --max-bytes N             Byte bound per show/hover request (default 16384, maximum 65536).
  --include-declaration     Include declarations in reference requests.

EXAMPLE
  pira_nav query --show src/app.py::App::run --range -2:-1 --references src/app.py::App::run"#),
        "languages" => r#"pira_nav languages — compiled code/document and LSP discovery support

USAGE
  pira_nav languages

Prints supported code languages with the discovered conventional PATH-LSP executable or `missing`, plus
native document formats. This command starts no server and performs no repository scan."#.into(),
        "help" => r#"pira_nav help — show detailed command help

USAGE
  pira_nav help COMMAND [COMMAND...]

Several topics may be requested together. `pira_nav COMMAND --help` is equivalent for one command."#
            .into(),
        other => return Err((2, crate::cli::unknown_command(other))),
    };
    writeln!(output, "{body}").map_err(output_error)
}
