use std::io::Write;

use crate::command::{CommandResult, output_error};
use crate::language::Language;

const GLOBAL_HELP_BODY: &str = r#"USAGE
  pira_codenav [LANGUAGE] SUBCOMMAND [ARGS...]
  pira_codenav help SUBCOMMAND

STRUCTURAL COMMANDS
  map DIRECTORY [--max-items N]  bounded repository shape
  find PATH QUERY...        ranked declaration lookup with bounded unique source
  search PATH PATTERN...    bounded implementation-text matches and context
  outline FILE...           declarations and ranges without bodies
  show TARGET...            exact selected source, line spans, or windows
  imports FILE...           direct import/include statements
  dependents FILE           direct reverse file dependencies
  deps FILE                 bounded transitive local file dependencies
  languages                 compiled language capabilities

LSP COMMANDS
  definition LOCATION...       semantic definitions
  implementation LOCATION...   concrete implementations
  type-definition LOCATION...  resolved type declarations
  references LOCATION...       semantic references
  callers LOCATION...          incoming call hierarchy
  callees LOCATION...          outgoing call hierarchy
  hover LOCATION...            bounded type or documentation text
  query OPERATION=LOCATION...  mixed semantic requests sharing LSP state

LANGUAGE AND LSP
  LANGUAGE is normally inferred from a suffix or shebang. Outline, map, find, and structural show
  targets require and prefer a conventional dedicated LSP discovered on PATH or supplied with
  --lsp [LANGUAGE=]ABSOLUTE_PATH. Pass --no-lsp to explicitly use bundled native parsing instead.
  Parser-free show spans/windows and text/file-dependency commands need no LSP. Semantic commands
  always require an LSP. Every server is invocation-local.

OUTPUT
  Output is bounded and deterministic. Predictable success fields are omitted. backend=lsp,
  complete=0, failed/error, omitted, and truncated fields appear only when relevant. Successful
  rows remain available when peer files or targets fail; an all-failed command returns an error.

SAFETY
  Repository source is read but never executed or edited. Exact source and hover text are framed as
  untrusted data. An explicit or PATH-discovered LSP is an external executable and may keep caches.

Run `pira_codenav SUBCOMMAND --help` for command syntax, options, defaults, and examples."#;

pub fn global(version: &str, output: &mut dyn Write) -> CommandResult {
    writeln!(
        output,
        "pira_codenav {version} — read-only code navigation\n"
    )
    .and_then(|()| writeln!(output, "{GLOBAL_HELP_BODY}"))
    .map_err(output_error)
}

pub fn language(language: Language, output: &mut dyn Write) -> CommandResult {
    writeln!(
        output,
        "pira_codenav {} — explicit language selection\n\nUSAGE\n  pira_codenav {} SUBCOMMAND [ARGS...]\n\nBEHAVIOR\n  Use this prefix for an extensionless or ambiguous file, to restrict map/find to {}, or to select\n  {}-qualified LSP options. A conflicting recognized suffix is an error. All commands and options\n  are otherwise unchanged.\n\nRun `pira_codenav SUBCOMMAND --help` for command syntax.",
        language.name(),
        language.name(),
        language.name(),
        language.name()
    )
    .map_err(output_error)
}

const OUTLINE_HELP: &str = r#"pira_codenav outline — inspect declarations without implementation bodies

DESCRIPTION
  Lists parsed declarations, signatures, ranges, and optional selectors for one or more files.

USAGE
  pira_codenav [LANGUAGE] outline FILE... [--match TEXT]... [--max-items N]
    [--signatures] [--selectors] [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]
    [--no-lsp]

OPTIONS
  --match TEXT      Case-insensitive OR filter over kind, qualified name, and signature.
                    Repeat it for alternatives. It is not a regex.
  --signatures      Add signature/type detail otherwise omitted.
  --selectors       Add freshness-checked identities for later `show`.
  --max-items N     Per-file declaration limit; default 1,000.

OUTPUT AND BACKEND
  Prints declaration kind, qualified name, and exact range; bodies are omitted. A PATH-discovered
  or explicit LSP is required by default and backend=lsp marks its result. On clean source, bundled
  parsing supplements declaration forms omitted by documentSymbol; LSP symbols remain primary.
  --no-lsp explicitly selects native-only parsing and rejects syntax-dirty input. Peer successes
  remain visible.

EXAMPLE
  pira_codenav outline src/parser.rs --match parse --signatures"#;

const SHOW_HELP: &str = r#"pira_codenav show — retrieve one exact structural item or line span

DESCRIPTION
  Prints source selected by position, qualified name, selector, line span, or line window.

USAGE
  pira_codenav [LANGUAGE] show TARGET... [--window N] [--max-items N] [--max-bytes N]
    [--lsp [LANGUAGE=]ABSOLUTE_PATH] [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]
    [--no-lsp]

TARGETS
  FILE:LINE[:COLUMN]       Selects the smallest enclosing named item; coordinates are one-based.
  FILE::QUALIFIED-NAME     Exact declaration name from outline/find.
  pira://...               Freshness-checked selector from --selectors.
  FILE:START-END           Exact inclusive parser-free line span; spans may be batched or mixed.

OPTIONS
  --window N               For one FILE:LINE[:COLUMN], return N lines before and after that line.
                           N may be zero; this bypasses structural item selection.

BOUNDS AND OUTPUT
  A single structural target returns the whole item by default. Multiple targets default to 20
  deduplicated items or spans and 32 KiB; --max-items and --max-bytes omit whole results rather than
  truncating source. Items over 200 lines carry a bounded-retrieval hint. FILE:START-END clamps END
  at EOF. Structural item targets require a PATH-discovered or explicit LSP by default; --no-lsp
  explicitly selects clean native parsing. Parser-free line spans and windows need no LSP.
  Selectors reject stale source. Returned source is framed as untrusted data.

EXAMPLES
  pira_codenav show src/parser.rs:120
  pira_codenav show src/parser.rs:120 --window 8
  pira_codenav show src/parser.rs::Parser::parse
  pira_codenav show src/parser.rs:120-145
  pira_codenav show src/a.rs:10-20 src/b.rs:30-40"#;

const MAP_HELP: &str = r#"pira_codenav map — produce a bounded repository or subsystem shape

DESCRIPTION
  Prints a bounded repository or subsystem shape.

USAGE
  pira_codenav [LANGUAGE] map DIRECTORY [--max-items N] [--lsp [LANGUAGE=]ABSOLUTE_PATH]...
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]
    [--no-lsp]

OUTPUT AND LIMITS
  Prints compact file rows with language and representative top-level declarations. Selection is
  deterministic and balanced across directories. Default: 200 files; pass a narrower DIRECTORY or
  raise --max-items deliberately for a larger pass. `map` does not use a depth option.

DISCOVERY AND BACKEND
  Git ignore rules are honored and symlinked directories are not followed. Without LANGUAGE, each
  supported file is inferred independently; LANGUAGE restricts the scan. Every discovered language
  needs a PATH or explicit LSP by default, and backend=lsp marks those rows. Clean bundled parsing
  supplements symbols omitted by documentSymbol. --no-lsp explicitly selects native-only parsing;
  syntax-dirty native files are reported as gaps.
  complete=0 and bounded errors identify gaps without discarding clean rows.

EXAMPLE
  pira_codenav map src --max-items 200"#;

const FIND_HELP: &str = r#"pira_codenav find — search declarations across source paths

DESCRIPTION
  Searches parsed declaration metadata in one file or across a directory; body text is not searched.

USAGE
  pira_codenav [LANGUAGE] find PATH QUERY... [--exact | --contains | --regex] [--kind KIND]
    [--max-items N] [--selectors] [--signatures] [--locations-only | --show-unique]
    [--lsp [LANGUAGE=]ABSOLUTE_PATH]...
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]
    [--no-lsp]

MATCHING
  Default            Case-insensitive full/suffix name first; substring fallback only if none exist.
  --exact            Case-insensitive full name or qualified-name suffix.
  --contains         Case-insensitive substring matching, including related names.
  --regex            Rust regex; case-sensitive unless the pattern requests otherwise.
  --kind KIND        Restrict declaration kind.
  --signatures       Add signature/type detail.
  --selectors        Include freshness-checked `show` targets.
  --locations-only   Omit automatic source. --show-unique explicitly requests the default behavior.
  QUERY...           One to 32 queries; files are parsed once for the whole batch.
  --max-items N      Per-query result limit; default 20; all query limits total at most 100,000.

OUTPUT AND BACKEND
  Bounded top-K results rank public/close names before hidden/distant matches, then use stable source
  order. Private results remain searchable. One match includes source up to 200 lines/24 KiB under a
  shared 32 KiB budget. Larger or ambiguous results provide bounded locations/selectors. Clean files
  use a PATH or explicit LSP by default; clean native symbols fill LSP omissions. --no-lsp selects
  native-only parsing. Peer successes remain visible.

EXAMPLES
  pira_codenav find . Module.compile compile_fx
  pira_codenav find . widget --contains --locations-only
  pira_codenav find src '^Parser::parse$' --regex --selectors"#;

const SEARCH_HELP: &str = r#"pira_codenav search — find implementation text with bounded context

DESCRIPTION
  Finds body text, operators, literals, or conditions in one file or directory with bounded source context.

USAGE
  pira_codenav [LANGUAGE] search PATH PATTERN... [--regex] [--context N]
    [--max-items N] [--max-bytes N]

MATCHING AND BOUNDS
  Literal matching is case-sensitive. --regex uses Rust regex syntax. One to 32 patterns are scanned
  together and reported as q1..q32. --context defaults to 1 line on each side and may be zero.
  --max-items limits matching lines (default 48, maximum 10,000); --max-bytes limits complete
  rendered context blocks (default 24 KiB). Overlapping windows merge without duplicating source.
  Put `--` before a literal or regex pattern beginning with `-`.

OUTPUT AND SCOPE
  Git ignore rules and LANGUAGE filtering match `find`. Exact contextual lines are framed as
  untrusted repository data. Only matched files are structurally parsed, solely to add the smallest
  clean enclosing item when available; text matches remain usable for syntax-dirty files.

EXAMPLES
  pira_codenav search . 'raise ' 'except ' --context 3
  pira_codenav search src 'if .*is_none' --regex --context 1"#;

const IMPORTS_HELP: &str = r#"pira_codenav imports — inspect direct import/include statements

DESCRIPTION
  Lists direct import/include statements and conservatively resolved local targets for files.

USAGE
  pira_codenav [LANGUAGE] imports FILE...

OUTPUT AND SCOPE
  Prints source line, exact import text, resolution status, and a conservative local target when one
  can be resolved from the current workspace. Run from the intended workspace root. External,
  dynamic, ambiguous, package-dependent, and build-dependent targets remain visibly unresolved.
  Multiple FILE successes survive peer failures.

BOUNDARY
  Requires clean native Tree-sitter syntax because LSP has no portable import graph.
  Never invokes a package manager or build system, and never executes a source file.

EXAMPLE
  pira_codenav imports src/app.py src/model.py"#;

const DEPENDENTS_HELP: &str = r#"pira_codenav dependents — inspect direct reverse file dependencies

DESCRIPTION
  Lists files whose imports/includes resolve directly to one local file.

USAGE
  pira_codenav [LANGUAGE] dependents FILE [--root DIRECTORY]

INPUT AND SCOPE
  FILE is relative to --root; --root defaults to the current directory. Narrow --root for large
  repositories. The scan uses the target language plus only the C/C++/CUDA or JavaScript/TypeScript
  compatibility group when applicable.

OUTPUT
  Prints each direct dependent with import line and resolution. complete=0 and bounded errors identify
  parse gaps. This is file dependency navigation, not symbol reference search. Clean native syntax is
  required because standard LSP has no portable import graph.

EXAMPLE
  pira_codenav dependents package/model.py --root src"#;

const DEPS_HELP: &str = r#"pira_codenav deps — traverse bounded local structural file dependencies

DESCRIPTION
  Traverses a bounded transitive local import/include graph in either or both directions.

USAGE
  pira_codenav [LANGUAGE] deps FILE [--direction imports|dependents|both] [--depth N]
    [--root DIRECTORY] [--max-items N]

INPUT AND OPTIONS
  FILE is relative to --root; it is not FILE:LINE or a symbol target.
  --direction VALUE   imports, dependents, or both; default both.
  --depth N           Traversal depth; default 2.
  --root DIRECTORY    Dependency workspace; default current directory.
  --max-items N       Shared edge limit; default 1,000. `both` alternates directions.

OUTPUT AND BOUNDARY
  Prints conservative local file edges with depth and direction. complete=0 marks parse gaps. It does
  not infer symbol references, calls, build-system edges, package resolution, or dynamic imports.
  Clean native syntax is required because standard LSP has no portable import graph.

EXAMPLE
  pira_codenav deps package/api.py --root src --direction both --depth 2"#;

const DEFINITION_HELP: &str = r#"pira_codenav definition — locate semantic definitions through LSP

DESCRIPTION
  Requests semantic definitions for source positions from an LSP server.

USAGE
  pira_codenav [LANGUAGE] definition FILE:LINE:COLUMN... [--max-items N]
    [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

INPUT AND OUTPUT
  LOCATION is FILE:LINE:COLUMN with one-based lines and UTF-8 byte columns. The default is 20
  locations per target. Local readable locations use PIRA coordinates; other locations retain LSP
  coordinates and encoding. No source body is printed.

LSP
  A conventional dedicated server on PATH is used automatically. --lsp with an absolute executable
  overrides discovery. --lsp-root selects the workspace.
  Up to 32 targets reuse one server and open document per file. Semantic results are never guessed.

EXAMPLE
  pira_codenav definition src/app.cpp:42:17 --lsp /usr/bin/clangd --lsp-root ."#;

const IMPLEMENTATION_HELP: &str = r#"pira_codenav implementation — locate semantic implementations through LSP

DESCRIPTION
  Requests concrete implementations for source positions from an LSP server.

USAGE
  pira_codenav [LANGUAGE] implementation FILE:LINE:COLUMN... [--max-items N]
    [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

INPUT AND OUTPUT
  LOCATION uses one-based lines and UTF-8 byte columns. Default: 20 normalized locations per target,
  no source bodies. Up to 32 targets reuse one server and document state. The server must advertise
  implementation support; this command never guesses. PATH discovery is automatic; --lsp overrides
  it.

EXAMPLE
  pira_codenav implementation src/api.py:18:12 --lsp /absolute/path/to/server --lsp-root ."#;

const TYPE_DEFINITION_HELP: &str = r#"pira_codenav type-definition — locate semantic type definitions through LSP

DESCRIPTION
  Requests resolved type declarations for source positions from an LSP server.

USAGE
  pira_codenav [LANGUAGE] type-definition FILE:LINE:COLUMN... [--max-items N]
    [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

INPUT AND OUTPUT
  LOCATION uses one-based lines and UTF-8 byte columns. Default: 20 normalized locations per target,
  no source bodies. Up to 32 targets reuse one server and document state. The server must advertise
  type-definition support; this command never guesses. PATH discovery is automatic; --lsp overrides
  it.

EXAMPLE
  pira_codenav type-definition src/app.ts:30:9 --lsp /absolute/path/to/server --lsp-root ."#;

const REFERENCES_HELP: &str = r#"pira_codenav references — locate bounded semantic references through LSP

DESCRIPTION
  Requests bounded semantic references for source positions from an LSP server.

USAGE
  pira_codenav [LANGUAGE] references FILE:LINE:COLUMN... [--include-declaration]
    [--max-items N] [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR] [--lsp-init [LANGUAGE=]JSON_FILE]
    [--lsp-settings [LANGUAGE=]JSON_FILE]

OPTIONS AND OUTPUT
  LOCATION uses one-based lines and UTF-8 byte columns. Declarations are excluded unless
  --include-declaration is passed. Default: 200 locations per target. Headers add shown/omitted only
  when bounded. Up to 32 targets reuse one server and document state. No source bodies are printed;
  this command never performs text search. PATH discovery is automatic; --lsp overrides it.

EXAMPLE
  pira_codenav references src/lib.rs:80:14 --max-items 50 --lsp /absolute/server --lsp-root ."#;

const CALLERS_HELP: &str = r#"pira_codenav callers — inspect incoming semantic calls through LSP

DESCRIPTION
  Requests incoming call-hierarchy relations for source positions from an LSP server.

USAGE
  pira_codenav [LANGUAGE] callers FILE:LINE:COLUMN... [--max-items N]
    [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

OUTPUT
  Default: 100 caller relations per target and 8 compact call sites per relation. Readable locations
  are normalized. Up to 32 targets reuse one server and document state. The server must support LSP
  call hierarchy; no textual or heuristic call graph is produced. PATH discovery is automatic;
  --lsp overrides it.

EXAMPLE
  pira_codenav callers src/app.cpp:42:17 --lsp /usr/bin/clangd --lsp-root ."#;

const CALLEES_HELP: &str = r#"pira_codenav callees — inspect outgoing semantic calls through LSP

DESCRIPTION
  Requests outgoing call-hierarchy relations for source positions from an LSP server.

USAGE
  pira_codenav [LANGUAGE] callees FILE:LINE:COLUMN... [--max-items N]
    [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

OUTPUT
  Default: 100 callee relations per target and 8 compact call sites per relation. Readable locations
  are normalized. Up to 32 targets reuse one server and document state. The server must support LSP
  call hierarchy; no textual or heuristic call graph is produced. PATH discovery is automatic;
  --lsp overrides it.

EXAMPLE
  pira_codenav callees src/app.cpp:42:17 --lsp /usr/bin/clangd --lsp-root ."#;

const HOVER_HELP: &str = r#"pira_codenav hover — retrieve bounded semantic type or documentation text through LSP

DESCRIPTION
  Requests bounded type, signature, or documentation text for source positions from an LSP server.

USAGE
  pira_codenav [LANGUAGE] hover FILE:LINE:COLUMN... [--max-bytes N]
    [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

INPUT AND OUTPUT
  LOCATION uses one-based lines and UTF-8 byte columns. --max-bytes defaults to 16 KiB per target.
  Truncation occurs only at a UTF-8 boundary and is reported in the header. Content is framed as
  untrusted LSP data. Up to 32 targets reuse one server and document state. PATH discovery is
  automatic; --lsp overrides it.

EXAMPLE
  pira_codenav hover src/app.py:24:9 --max-bytes 4096 --lsp /absolute/server --lsp-root ."#;

const QUERY_HELP: &str = r#"pira_codenav query — run mixed semantic requests with shared LSP state

DESCRIPTION
  Runs bounded definition, implementation, type-definition, references, hover, callers, and
  callees requests while reusing each matching language server and open-document state.

USAGE
  pira_codenav [LANGUAGE] query OPERATION=FILE:LINE:COLUMN... [--max-items N]
    [--max-bytes N] [--include-declaration] [--lsp [LANGUAGE=]ABSOLUTE_PATH]
    [--lsp-arg [LANGUAGE=]ARG]... [--lsp-root DIR]
    [--lsp-init [LANGUAGE=]JSON_FILE] [--lsp-settings [LANGUAGE=]JSON_FILE]

OPTIONS AND OUTPUT
  Up to 32 requests run in input order. --max-items overrides each location/call operation's normal
  bound and is capped at 1,000 per request; --max-bytes controls hover, defaults to 16 KiB, and is
  capped at 64 KiB. --include-declaration applies to
  references. Existing per-operation output formats are preserved, followed by one query status
  row. Successful requests remain visible when peers fail.

EXAMPLE
  pira_codenav query definition=src/app.py:24:9 hover=src/app.py:24:9 \
    references=src/app.py:24:9 --max-items 50 --lsp-root ."#;

const LANGUAGES_HELP: &str = r#"pira_codenav languages — list installed language capabilities

DESCRIPTION
  Lists language parsers compiled into this executable.

USAGE
  pira_codenav languages

OUTPUT
  Prints the supported-language count followed by one canonical LANGUAGE name per line. These names
  may prefix commands and qualify LSP options.

EXAMPLE
  pira_codenav languages"#;

pub fn command(command: &str, output: &mut dyn Write) -> CommandResult {
    let text = match command {
        "outline" => OUTLINE_HELP,
        "show" => SHOW_HELP,
        "map" => MAP_HELP,
        "find" => FIND_HELP,
        "search" => SEARCH_HELP,
        "imports" => IMPORTS_HELP,
        "dependents" => DEPENDENTS_HELP,
        "deps" => DEPS_HELP,
        "definition" => DEFINITION_HELP,
        "implementation" => IMPLEMENTATION_HELP,
        "type-definition" => TYPE_DEFINITION_HELP,
        "references" => REFERENCES_HELP,
        "callers" => CALLERS_HELP,
        "callees" => CALLEES_HELP,
        "hover" => HOVER_HELP,
        "query" => QUERY_HELP,
        "languages" => LANGUAGES_HELP,
        other => {
            return Err((
                2,
                format!("unknown subcommand `{other}`; run pira_codenav --help"),
            ));
        }
    };
    writeln!(output, "{text}").map_err(output_error)
}
