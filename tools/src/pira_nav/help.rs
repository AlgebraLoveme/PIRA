use std::io::Write;

use crate::command::{CommandResult, output_error};

const GLOBAL: &str = r#"USAGE
  pira_nav COMMAND [OPTIONS] [ARGUMENTS]
  pira_nav help COMMAND [COMMAND...]

COMMANDS
  map [PATH]                     Repository shape, documents, and declarations.
  search -e PATTERN... [PATH...] Bounded search over ordinary unignored UTF-8 text.
  symbols QUERY [PATH...]        Declarations, keys, or headings (`declarations` alias).
  outline FILE...                Declarations or document paths without bodies.
  show TARGET...                 Exact items, position windows, or line ranges.
  imports | dependents | deps    Conservative syntax-level file relationships.
  definition | implementation | type-definition | references
                                  LSP symbol locations.
  callers | callees | supertypes | subtypes
                                  LSP call or type hierarchy.
  hover                           Bounded LSP hover text.
  query                           Mixed semantic requests sharing LSP state.
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
  pira_nav map [PATH] [--max-items N] [OPTIONS]

PATH defaults to `.`. Output includes compact code/document counts, top directories, generic project
landmarks, and balanced representative declarations or top-level keys. At most 20 navigable rows are
shown by default. Ignored and hidden paths are outside directory scope; symlinked directories are not
followed. Recognizable fixture/corpus subtrees are counted but skipped by broad maps; map one directly
to inspect it structurally.

OPTIONS
{STRUCTURAL_OPTIONS}
  --max-items N             Maximum representative file rows (default 20).

EXAMPLE
  pira_nav map src"#),
        "search" => r#"pira_nav search — bounded portable repository text search

USAGE
  pira_nav search PATTERN [PATH...] [OPTIONS]
  pira_nav search -e PATTERN [-e PATTERN]... [PATH...] [OPTIONS]

PATH defaults to `.`; up to 64 overlapping paths are deduplicated. Directories cover ordinary
unignored UTF-8 text, including unsupported languages; explicit files bypass ignore discovery. With
several paths, missing peers mark output incomplete while valid peers are searched; one missing target
is an error. Binary, non-UTF-8, oversized, and unreadable files are aggregate incomplete counts.

OPTIONS
  -e, --pattern PATTERN     Add a pattern (1..32 total).
  --language LANGUAGE       Restrict search to one supported language.
  -F, --fixed-strings       Match literals (the default); conflicts with --regex.
  --regex                   Use bounded Rust-regex syntax.
  -i, --ignore-case         Match without case distinctions.
  -w, --word                Require Unicode half-word boundaries.
  -l, --files-with-matches  Print matching paths and query coverage only.
  -c, --count               Print exact matching-line counts per file/query.
  -C, --context N           Context lines for snippets (default 2, maximum 1000).
  --max-items, --max-results N  Maximum shown lines/file rows (default 48).
  --max-per-query N         Maximum snippet lines selected per pattern (default 8).
  --max-bytes N             Maximum rendered source-block bytes (default 8192).
  --owners                  Annotate snippet matches with enclosing clean declarations when available.

One scan serves all patterns. Each ranks independently, preferring declaration-like production hits;
batches remain query-balanced with exact shown/omitted counts. Zero matches is success. Source is
framed as untrusted data; an overlong matching line is identified but not truncated.

EXAMPLES
  pira_nav search Parser src
  pira_nav search -e Parser -e Compiler src
  pira_nav search 'impl\\s+Parser' --regex --word"#.into(),
        "symbols" => format!(r#"pira_nav symbols — ranked declaration and key discovery

USAGE
  pira_nav symbols QUERY [PATH...] [OPTIONS]
  pira_nav symbols --query QUERY [--query QUERY]... [PATH...] [OPTIONS]

PATH defaults to `.`; up to 64 overlapping paths are deduplicated. Searches code declaration names,
JSON/JSONC/YAML/TOML key paths, and Markdown headings, not arbitrary body text. With several paths,
missing peers mark output incomplete while valid peers are searched. Unique matches include bounded
exact source by default; use --locations-only to suppress it.

OPTIONS
{STRUCTURAL_OPTIONS}
  --query QUERY             Add a declaration/key query (1..32 total).
  --exact | --contains | --regex
  --kind KIND
  --max-items N             Rows per query (default 20).
  --selectors               Include freshness-checked selectors.
  --signatures              Include bounded signatures.
  --show-unique | --locations-only

EXAMPLE
  pira_nav symbols Parser src tests"#),
        "outline" => format!(r#"pira_nav outline — declarations or document paths without bodies

USAGE
  pira_nav outline FILE... [OPTIONS]

OPTIONS
{STRUCTURAL_OPTIONS}
  --max-items N             Maximum items across the invocation (default 64).
  --depth N                 Maximum nested depth; 0 shows top-level items only.
  --match QUERY             Restrict declarations; repeatable.
  --signatures              Include bounded declaration signatures.
  --selectors               Include freshness-checked selectors.

EXAMPLE
  pira_nav outline src/parser.rs
  pira_nav outline .github/workflows/test.yaml
  pira_nav outline README.md"#),
        "show" => format!(r#"pira_nav show — exact bounded source retrieval

USAGE
  pira_nav show TARGET... [OPTIONS]

TARGET is FILE::QUALIFIED-NAME, a unique qualified-name suffix, pira://selector, FILE:LINE[:COLUMN], or
FILE:START-END. A position selects the smallest enclosing named item; --window N selects parser-free surrounding lines. Exact line ranges
and windows work for any readable UTF-8 text file without language inference. Source is printed exactly
apart from terminal-control escaping and is framed as untrusted data.

Structured-document names use dot-separated keys, [N] array indices, and JSON-quoted brackets for
special keys, for example ["a.b"]. Markdown names use `Parent > Child`. Shell-quote targets containing
brackets, `>`, or other metacharacters.

OPTIONS
{STRUCTURAL_OPTIONS}
  --window N
  --max-items N
  --max-bytes N

EXAMPLES
  pira_nav show src/parser.rs::Parser::parse
  pira_nav show 'package.json::scripts.build'
  pira_nav show 'workflow.yaml::jobs.test.steps[2]'
  pira_nav show 'README.md::Install > Linux'
  pira_nav show src/parser.rs:120-160"#),
        "imports" => r#"pira_nav imports — syntax-level file imports

USAGE
  pira_nav imports FILE... [--language LANGUAGE] [--max-items N]

Parses import/include syntax and conservatively resolves local paths inside the workspace. External,
unresolved, and blocked edges remain explicit. It never invokes a package manager or build system."#.into(),
        "dependents" => r#"pira_nav dependents — reverse local import lookup

USAGE
  pira_nav dependents FILE [--root ROOT] [--language LANGUAGE]

ROOT defaults to the current directory. FILE is resolved from the current directory, then ROOT when
the first path does not exist, and must lie within ROOT. Every eligible file is scanned; syntax failures
and unresolved relationships are accounted separately."#.into(),
        "deps" => r#"pira_nav deps — bounded local dependency traversal

USAGE
  pira_nav deps FILE [--root ROOT] [--direction imports|dependents|both] [--depth N]
      [--language LANGUAGE] [--max-items N]

FILE is resolved from the current directory, then ROOT when the first path does not exist, and must lie
within ROOT. Traverses only conservatively resolved local syntax edges. This is not a build graph.
Defaults: direction=both, depth=2, max-items=128."#.into(),
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
coordinates. Qualified names must resolve uniquely; selectors are freshness-checked. Results are bounded
and peer successes survive per-target failures.

OPTIONS
{LSP_OPTIONS}
  --max-items N{extra}

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
  --max-bytes N             Bytes per hover (default 16384)."#),
        "query" => format!(r#"pira_nav query — ordered mixed semantic requests with shared LSP state

USAGE
  pira_nav query --definition TARGET [--hover TARGET]... [OPTIONS]

Operation options may repeat and execute in command-line order: --definition, --implementation,
--type-definition, --references, --callers, --callees, --supertypes, --subtypes, and --hover. Up to 32
requests reuse invocation-local servers, open documents, source buffers, and position indices.

OPTIONS
{LSP_OPTIONS}
  --max-items N             Shared row bound per non-hover request.
  --max-bytes N             Shared byte bound per hover request.
  --include-declaration     Include declarations in reference requests.

EXAMPLE
  pira_nav query --definition src/app.py::App.run --hover src/app.py::App.run"#),
        "languages" => r#"pira_nav languages — compiled code/document and LSP discovery support

USAGE
  pira_nav languages

Prints supported code languages with the discovered conventional PATH-LSP executable or `missing`, plus
native document formats. This command starts no server and performs no repository scan."#.into(),
        other => return Err((2, format!("unknown subcommand `{other}`; run pira_nav --help"))),
    };
    writeln!(output, "{body}").map_err(output_error)
}
