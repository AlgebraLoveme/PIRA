pub const GLOBAL: &str = r#"pira_ctx bounds command output and retains compacted output for exact local recovery.

Choosing a command:
  Run a PROGRAM:
    auto       Default; name optional. Print short output or retain it and return a compact view.
    check      Return only PASS/FAIL and child status.
    exact      Request original output; highly repetitive non-interactive output may be retained.
    capture    Always retain output up to the configured space ceiling (`summary` is an alias).
    batch      Run several independent intent-tagged commands.

  Inspect a retained RESULT:
    search     Locate wording; start here for targeted evidence.
    range      Return the smallest sufficient exact line range.
    transform  Deterministic filtering, counting, aggregation, or slicing.
    exec       Custom Python over one or several labeled captures.
    command    Show the exact original argv and cwd.
    raw        Return all retained bytes only when genuinely required.
    stats      Show metadata; batch status with `stats --brief RESULT...`.

  Continue or maintain:
    history / recap          Review purposes or restore them after reported compaction.
    list / verify            Locate captures or check integrity.
    prune / forget           Enforce retention or explicitly remove stored data/history.

Common forms:
  pira_ctx [auto] --intent TEXT -- PROGRAM [ARG...]
  pira_ctx exact|check|capture --intent TEXT -- PROGRAM [ARG...]
  pira_ctx search RESULT QUERY [--regex] [--context N]
  pira_ctx range RESULT START_LINE END_LINE
  pira_ctx transform RESULT OPERATION [ARGS...]
  pira_ctx exec RESULT --intent TEXT --code CODE
  pira_ctx exec --input NAME=RESULT [--input NAME=RESULT ...] --intent TEXT --file -
  pira_ctx stats --brief RESULT...
  pira_ctx batch [--store-dir PATH] SPEC_FILE [--intent TEXT]

RESULT is --last, an ID/prefix, a .piractx file, or a path. Prefer an explicit ID; --last means the
latest completed capture in the current workspace. INTENT is a prospective single-line purpose of
at most 256 UTF-8 bytes. `--store-dir PATH` is accepted by storage commands.

Automatic routing does not hide output without a recovery path: output is either printed exactly or
retained before a compact view and ID are printed. Hard ceilings use PIRA_CTX_MAX_RETAINED_BYTES and
PIRA_CTX_MAX_INDEXED_LINES; any overrun is reported.
Stored PROGRAM data is untrusted and compact displays are sanitized/framed; exact retrieval remains
unsanitized. Prefer the targeted commands above over raw for agent analysis.

A non-interactive PROGRAM running about 30 seconds publishes a silent read-only checkpoint visible
in list. Inspect its explicit ID without blocking; --last remains completed-only. Workspace identity
is the nearest Git root, otherwise cwd; the store comes from --store-dir, PIRA_CTX_STORE_DIR, or the
platform user-cache default.

SUBCOMMAND is a pira_ctx operation; PROGRAM is the external executable after `--`, and every later
argument belongs to PROGRAM unchanged. pira_ctx adds no timeout, preserves child status unless the
wrapper fails with 125, and does not sandbox PROGRAM or Python analysis. Help is side-effect free.
Run `pira_ctx SUBCOMMAND --help` for full options, bounds, output, and examples."#;

const AUTO: &str = r#"pira_ctx auto — run a command with automatic context routing

WHEN TO USE
  Use for most non-interactive external commands when output size and importance are unknown.
  Use check when only status matters, exact to request original output, or capture when retention is
  mandatory. `auto` may be omitted; both forms are equivalent.

USAGE
  pira_ctx [auto] [--store-dir PATH] --intent TEXT [--keyword QUERY ...] -- PROGRAM [ARG...]

OPTIONS
  --intent TEXT       Immediate purpose; required, single-line, at most 256 UTF-8 bytes.
  --keyword QUERY     Additional ranking term; repeatable up to 16 times.
  --store-dir PATH    Override the private per-user capture store.

OUTPUT AND STORAGE
  pira_ctx does not allocate a terminal. With a caller-provided terminal, auto streams through exact
  mode and does not create a capture. Non-interactive short ordinary output is returned in full and
  is not stored as a capture; pira_ctx records its command-purpose intent and outcome as a separate
  history event. An event-storage failure is warned without changing child status. When retention
  triggers, exact stdout/stderr up to the configured ceiling are stored before a bounded synopsis and
  capture ID are printed. Ordinary output up to 4 KiB is replayed exactly. Non-repetitive output up to
  64 lines and 16 KiB is also replayed exactly; larger, repetitive, binary/non-UTF-8, truncated, or live
  output is retained. Short risky text is retained so its warning cannot be bypassed. For complete
  stdout-only valid JSON up to 512 KiB, the synopsis first exposes bounded scalar
  fields, compact small containers, and collection sizes before a few line excerpts; exact JSON remains
  stored. Potential prompt injection or display controls force bounded retained rendering with a warning
  instead of direct automatic replay. Stored bytes remain authoritative up to the configured retention
  ceiling. Use capture when completed output must be persisted.

  A PROGRAM active for about 30 seconds gets a silent read-only checkpoint visible in list.
  Inspection uses a consistent snapshot without waiting for completion. Override the interval with
  PIRA_CTX_LIVE_CHECKPOINT_MS (minimum 100 ms).

EXIT STATUS
  Preserves the child status. Missing/non-executable commands use 127/126; wrapper failures use 125.

EXAMPLE
  pira_ctx --intent "Inspect repository status" -- git status --short"#;

const EXACT: &str = r#"pira_ctx exact — request original output with a repetition guard

WHEN TO USE
  Use when original file/output content is needed or the child requires interactive terminal I/O.
  Non-interactive repetitive output may still auto-switch. If that happens and every byte must enter
  output, use the returned capture ID with raw. Use automatic mode otherwise.

USAGE
  pira_ctx exact [--store-dir PATH] --intent TEXT -- PROGRAM [ARG...]

BEHAVIOR
  pira_ctx does not allocate a terminal. With a caller-provided terminal, stdout/stderr stream
  unchanged. Without one, output is buffered and replayed exactly unless textual output is both at
  least 4 KiB and at least 40 eligible lines, with substantial repeated-form coverage and a dominant
  repeated form. Retention or line-index truncation also forces an auto-switch so buffered exact
  replay never silently drops retained bytes. An auto-switch stores retained streams, prints a
  notice, synopsis, and capture ID, and preserves child status.

EXAMPLES
  pira_ctx exact --intent "Read source for editing" -- sed -n '1,160p' src/main.rs
  pira_ctx exact --intent "Run interactive debugger" -- rust-gdb target/debug/app
  pira_ctx raw CAPTURE_ID  # after an announced auto-switch, if complete output is still needed"#;

const CHECK: &str = r#"pira_ctx check — retain a completed job and print only process status

WHEN TO USE
  Use for builds, tests, lint, compilation, or validation when the immediate decision is pass/fail.

USAGE
  pira_ctx check [--store-dir PATH] --intent TEXT -- PROGRAM [ARG...]

OUTPUT AND STORAGE
  Every completed child is retained, including empty or short output. Active output is one line:
    PASS|FAIL | exit=CODE | duration=Nms | result=ID
  PASS/FAIL depends only on child exit status; it does not independently verify the PROGRAM's claim.
  Spawn failures print result=- and have no capture.

EXIT STATUS
  Preserves the child status. Missing/non-executable commands use 127/126; wrapper failures use 125.

EXAMPLE
  pira_ctx check --intent "Verify the Rust test suite" -- cargo test --locked"#;

const CAPTURE: &str = r#"pira_ctx capture — always retain completed command output and return a synopsis

WHEN TO USE
  Use when output retention is mandatory up to the configured space ceiling.
  Use automatic mode when unconditional retention is unnecessary. `summary` is an alias.

USAGE
  pira_ctx capture [--store-dir PATH] --intent TEXT [--keyword QUERY ...] -- PROGRAM [ARG...]

OUTPUT AND STORAGE
  Every completed child is stored with retained stdout/stderr, metadata, indexes, compression, and
  integrity hashes. A bounded extractive synopsis and capture ID are printed, even for empty output.
  If the configured byte ceiling is reached, excess output is drained without storage and the report
  states the observed and retained sizes. Spawn failures have no capture. Child status is preserved.

EXAMPLE
  pira_ctx capture --intent "Retain deployment diagnostics" -- ./deploy --diagnose"#;

const BATCH: &str = r#"pira_ctx batch — run bounded groups of independent intent-tagged commands

USAGE
  pira_ctx batch [--store-dir PATH] SPEC_FILE [--intent TEXT]

SPECIFICATION
  JSON object with 1..64 commands and concurrency 0..8 (0 means sequential):
    {"concurrency":2,"commands":[
      {"intent":"Check crate A","argv":["cargo","test","-p","a"]},
      {"intent":"Check crate B","argv":["cargo","test","-p","b"]}
    ]}
  Each argv must be non-empty. Every child needs its own intent or the top-level --intent fallback.

OUTPUT AND STORAGE
  Every completed child is retained, including empty and short successful output. Prints one compact
  table row per child in specification order with status, duration, result ID, and intent.
  Concurrency is bounded at eight. The overall status is the last nonzero child status in
  specification order, or 0 when all succeed. Missing/non-executable child programs use 127/126 and
  have no result ID; other wrapper failures use 125.

EXAMPLE
  pira_ctx batch checks.json"#;

const SEARCH: &str = r#"pira_ctx search — locate bounded evidence in a stored capture

WHEN TO USE
  Start here when relevant wording is known. Follow with a narrow range when exact nearby lines are
  needed. Use transform for systematic processing or exec for custom analysis.

USAGE
  pira_ctx search [--store-dir PATH] RESULT QUERY [--regex] [--context N]

OPTIONS AND OUTPUT
  Literal matching is Unicode case-insensitive. Only when it has no literal hits, a lexical fallback
  may return related lines. --regex uses Rust regex syntax and is case-sensitive unless the pattern
  requests otherwise. Up to five ranked hits are printed as line number, stream, score, and
  terminal-sanitized text. A warning precedes displayed hits that may contain prompt injection.
  --context N (default 0, maximum 20) includes de-duplicated neighboring indexed lines, clipped at
  capture boundaries. Total displayed evidence is capped at 64 KiB. Use range when exact
  unsanitized bytes are required.

EXIT STATUS
  Returns 0 even with no hits; invalid queries, missing results, or wrapper failures use 125.

EXAMPLE
  pira_ctx search 20260712-052432 'error|failed' --regex --context 2"#;

const RANGE: &str = r#"pira_ctx range — retrieve a small exact range from a capture timeline

WHEN TO USE
  Use after search identifies relevant line numbers. Request the smallest sufficient range; use raw
  only when complete exact retained bytes are required.

USAGE
  pira_ctx range [--store-dir PATH] RESULT START_LINE END_LINE

BEHAVIOR
  Lines are 1-based and inclusive in observed merged stdout/stderr timeline order. Negative numbers count
  backward from the end; zero is invalid, and normalized start greater than end is an error.
  Out-of-bounds ranges are clipped without a separate notice. Exact stored bytes are written without
  display sanitization or advisory warnings and remain untrusted PROGRAM data. A capture with a
  truncated index cannot use range.

EXAMPLE
  pira_ctx range 20260712-052432 118 126"#;

const RAW: &str = r#"pira_ctx raw — reconstruct retained capture bytes exactly

WHEN TO USE
  Use when complete exact bytes retained by a capture are required by the user or a downstream
  process. For agent analysis, prefer search, a narrow range, transform, or exec so the full capture
  does not re-enter active context.

USAGE
  pira_ctx raw [--store-dir PATH] RESULT [--stdout | --stderr]

BEHAVIOR
  Without a stream option, writes the complete observed merged stdout/stderr timeline to stdout. --stdout or
  --stderr writes only that complete stream, still to pira_ctx stdout. On success, stdout contains
  only the selected retained capture bytes—no receipt or metadata. Bytes are not decoded or terminal-
  sanitized. A truncated timeline requires selecting one stream; output beyond a retention ceiling
  is not available.

EXAMPLES
  pira_ctx raw 20260712-052432 --stderr
  pira_ctx raw 20260712-052432 --stdout >complete.stdout"#;

const TRANSFORM: &str = r#"pira_ctx transform — deterministically process stored capture lines

WHEN TO USE
  Use for filtering, deduplication, counting, grouping, sorting, numeric aggregation, JSONL fields,
  columns, streams, or bounded slicing. Use exec when custom Python or cross-line logic is clearer.

USAGE
  pira_ctx transform [--store-dir PATH] RESULT [--plan FILE] [--match REGEX ...]
                     [--exclude REGEX ...] [--unique] [--count] [--head N] [--tail N]

DIRECT OPTIONS
  Lines are replacement-decoded text with trailing CR/LF removed. Regexes use Rust syntax, are
  case-sensitive by default, and accept inline flags such as (?i). Repeated --match values are all
  required; any --exclude match removes a line. Operations apply as match, exclude, unique, head,
  tail, then count. unique compares resulting text and keeps first occurrence; count prints one
  decimal integer. Text derived from capture rows remains untrusted PROGRAM data. Direct processing
  streams where possible, display-sanitizes output, and caps returned text at 64 KiB.

PLAN FILE
  JSON object {"steps":[STEP,...]}; steps run in order after CLI filters. Valid STEP objects:
    {"op":"match|exclude","regex":"..."}
    {"op":"context","regex":"...","before":N,"after":N}
    {"op":"head|tail|top","n":N} | {"op":"sort","numeric":true|false}
    {"op":"json_field","field":"name"} | {"op":"json_eq","field":"name","value":JSON}
    {"op":"column","index":N,"delimiter":"..."} | {"op":"stream","stream":"stdout|stderr"}
    {"op":"unique|count|group_count|sum|min|max|mean|diagnostic"}
  Numeric reductions use Rust f64 parsing/formatting, fail on nonnumeric text, and preserve accepted
  non-finite values. Malformed JSONL is an error for json_field; strings emit their contents, other
  JSON values emit compact JSON, and absent fields emit an empty string. json_eq treats malformed
  JSONL as nonmatching. column index is zero-based and delimiter defaults to tab. Plans materialize
  at most 1,000,000 rows and 128 MiB of exact uncompressed line bytes. A plan is at most 1 MiB and
  64 steps; context before/after values are at most 10,000. Parse/limit failures exit 125.

EXAMPLES
  pira_ctx transform RESULT --match 'FAILED|ERROR' --count
  pira_ctx transform RESULT --plan analysis.json
  analysis.json: {"steps":[{"op":"json_field","field":"value"},{"op":"sum"}]}"#;

const EXEC: &str = r#"pira_ctx exec — analyze a stored capture with explicit Python 3 code

WHEN TO USE
  Use for substantial or custom analysis not covered clearly by transform. Print only the result
  needed for the current decision: aggregate large collections and prefer counts/coordinates over
  matching source text unless that text is itself the answer. Retrieve a narrow unresolved
  diagnostic afterward. Analysis output itself follows non-interactive automatic routing.

USAGE
  pira_ctx exec [--store-dir PATH] RESULT --intent TEXT
                (--code CODE | --file PATH) [--python PATH]
  pira_ctx exec [--store-dir PATH] --input NAME=RESULT [--input NAME=RESULT ...]
                --intent TEXT (--code CODE | --file PATH) [--python PATH]

BINDINGS
  CAPTURES            Ordered mapping from each input name to a record. Read content as
                      CAPTURES[name]["text"] or ["bytes"]. Other keys are path, stdout_path,
                      stderr_path, id, exit, state, and generation.
  CAPTURE_NAMES       Input names in command order.
  MSGS                Merged texts in command order; MSG_BYTES_LIST and MSG_IDS are parallel lists.
  MSG...              The scalar bindings below exist only for a single RESULT/input.
  MSG                 Merged text with invalid UTF-8 replaced by U+FFFD.
  MSG_BYTES           Exact merged bytes.
  MSG_PATH            Private temporary merged-capture path.
  MSG_STDOUT_PATH     Private temporary exact-stdout path.
  MSG_STDERR_PATH     Private temporary exact-stderr path.
  MSG_ID              Resolved source capture ID.
  MSG_EXIT            Source command exit code, or None for a running checkpoint.
  MSG_STATE           `running` or `complete`.
  MSG_GENERATION      Live checkpoint generation, or 0 for a completed capture.

BEHAVIOR
  Choose one RESULT or up to 32 unique labeled --input values. Every target resolves once before
  execution. Combined materialization defaults to a 64 MiB ceiling controlled by
  PIRA_CTX_MAX_EXEC_BYTES. Choose exactly one code source; --file - reads bounded code from stdin
  and avoids shell quoting for multiline Python. Interpreter order is
  --python PATH, PIRA_CTX_PYTHON, python3, Windows `py -3`, then python. Python is optional for all
  other commands. Exact bytes and decoded text are eagerly loaded. Prefer search/transform for
  larger inputs or raise the ceiling deliberately. Temporary paths exist only during execution.
  Every running input is copied once before Python starts, so later PROGRAM writes cannot change the
  analysis view or be changed by it. Analysis code is limited to 1 MiB. Analysis status is preserved;
  retained analysis metadata links to all source IDs. Code runs with caller permissions and is not
  sandboxed.

EXAMPLES
  pira_ctx exec --last --intent "Count failures" --code 'print(MSG.count("FAILED"))'
  pira_ctx exec --input build=ID1 --input tests=ID2 --intent "Compare failures" --file - <<'PY'
  print({name: item["text"].count("FAILED") for name, item in CAPTURES.items()})
  PY
  pira_ctx exec RESULT --intent "Extract errors" --file analysis.py"#;

const RECAP: &str = r#"pira_ctx recap — restore recent same-thread command events after compaction

WHEN TO USE
  Run only after the platform reports compaction of the continuing thread and before further
  substantive shell/exec work. Do not use for a new or temporary thread. Recap covers only commands
  routed through pira_ctx; verify live state when that distinction matters.

USAGE
  pira_ctx recap [--store-dir PATH] [--limit N]

OUTPUT
  Prints the newest bounded current-thread events as a <pira_context_restore> block. Each row contains
  age, intent, child exit code, and a result ID when output was retained. Events are chronological.
  Command text and PROGRAM-derived content are omitted; inspect a result ID only when more detail is
  needed. --limit accepts 0..20 and defaults to 20; output is below 8 KiB. If no supported thread
  identifier is available, scope is labeled current-workspace-fallback rather than claiming same-
  thread recovery.

EXAMPLE
  pira_ctx recap --limit 10"#;

const HISTORY: &str = r#"pira_ctx history — review bounded command-purpose event history

WHEN TO USE
  Use when uncertain which command purposes were attempted or how they exited, especially before
  repeating expensive or state-changing work. With QUERY, only matching agent-supplied intents are
  shown. Use recap only after platform-reported compaction of the continuing thread. Keep durable
  project state, decisions, validated results, and reusable lessons in AGENT_WORKBOOK.md.

USAGE
  pira_ctx history [--store-dir PATH] [QUERY] [--regex] [--scope current|workspace]
                   [--since TIME] [--until TIME] [--offset N]
                   [--lookback N|all] [--limit N] [--details]

MATCHING AND SCOPE
  Without QUERY, newest events are shown. Literal QUERY matching lowercases Unicode text before
  substring comparison; it is not fuzzy, normalized, or semantic search. --regex uses Rust regex
  syntax and is case-sensitive unless the pattern requests otherwise. Filters inspect only intent.
  --scope current is the default and uses PIRA_CTX_THREAD_ID, then CODEX_THREAD_ID, then
  CLAUDE_CODE_SESSION_ID, without storing any raw value. With none, it uses a labeled
  workspace-local unscoped fallback that cannot guarantee thread isolation. --scope workspace
  explicitly merges anonymous thread catalogs.

BOUNDS AND OUTPUT
  Search covers all retained events by default and always returns a bounded result. --limit N stops
  after the newest N matches (default 10, range 1..100). --since TIME includes events at or after its
  bound; --until TIME excludes events at or after its bound. TIME is RFC 3339, `now`, or an age such
  as 30m, 24h, or 7d. --offset N skips the newest N events inside the time window before matching;
  --lookback N examines only the next N events, while --lookback all is the default. Number bounds are
  applied before QUERY. N is at most 8000.

  Rows contain age, exit status, optional result ID, and terminal-sanitized intent; workspace scope
  also shows an anonymous thread label. --details additionally reads selected records for duration
  and redacted command. The header reports how many events were examined and whether search stopped
  at the result limit; history_hits is exact only when complete=1. Selected authoritative records are
  checksum-validated. Retention keeps at most 2,000 events per thread and 8,000 per workspace, so
  `all` means all retained history rather than durable project memory.

EXIT STATUS
  Returns 0 even with no matches. Invalid queries or regexes and wrapper failures use 125.

EXAMPLES
  pira_ctx history --limit 10
  pira_ctx history build --since 2026-07-14T00:00:00+08:00 --until 2026-07-15T00:00:00+08:00
  pira_ctx history parser --scope workspace --offset 2000 --limit 10
  pira_ctx history 'C sharp parser' --lookback 300 --limit 10
  pira_ctx history 'build|test' --regex --scope workspace --details --limit 5"#;

const STATS: &str = r#"pira_ctx stats — show workspace totals or capture metadata

USAGE
  pira_ctx stats [--store-dir PATH] [RESULT]
  pira_ctx stats [--store-dir PATH] --brief RESULT...

OUTPUT
  Without RESULT, prints current-workspace capture totals, current-thread event count and scope,
  ignored legacy-event count, and workspace hash. With one RESULT, prints complete capture metadata;
  use --brief with up to 32 results when only state, exit, duration, and retained size are needed.
  Brief mode omits command, paths, format, index, and suggestions. Neither form prints captured
  content. A running result reports unknown exit status.

EXAMPLES
  pira_ctx stats
  pira_ctx stats --last
  pira_ctx stats --brief RESULT_A RESULT_B"#;

const COMMAND: &str = r#"pira_ctx command — retrieve the original invocation for a capture

USAGE
  pira_ctx command [--store-dir PATH] RESULT

OUTPUT
  Prints one JSON object with argv, cwd, and exact. New captures retain the original argument vector
  and report exact=true. Older captures fall back to their redacted argument vector and report
  exact=false. Running checkpoints are supported.

SECURITY
  argv and cwd are returned exactly and may contain secrets or private paths. Use this targeted
  command only when invocation traceability is needed; list and stats remain redacted.

EXAMPLE
  pira_ctx command 20260712-052432"#;

const VERIFY: &str = r#"pira_ctx verify — verify a stored capture's structure and stream integrity

USAGE
  pira_ctx verify [--store-dir PATH] RESULT

BEHAVIOR
  Validates the container layout, authenticated metadata/index/block tables, and exact stdout/stderr
  hashes supported by its format. Prints the verified path on success and does not modify the capture.
  Running checkpoints have no final hashes and are rejected until PROGRAM exits. Corruption, missing
  results, or wrapper failures use exit 125.

EXAMPLE
  pira_ctx verify 20260712-052432"#;

const LIST: &str = r#"pira_ctx list — list stored captures

USAGE
  pira_ctx list [--store-dir PATH] [--workspace current] [--limit N]

OUTPUT
  Prints up to 20 newest-first rows with ID, state, timestamp, exit status, bytes, lines, and a
  redacted command clipped to 256 bytes. Active checkpoints are marked running and use `-` as exit
  status. --limit accepts 0..100. Without --workspace current, entries from every workspace in the
  selected store are considered.

EXAMPLE
  pira_ctx list --workspace current"#;

const PRUNE: &str = r#"pira_ctx prune — enforce capture age or total-storage limits

USAGE
  pira_ctx prune [--store-dir PATH] [--max-age-days N] [--max-store-bytes N] [--legacy-events]

BEHAVIOR
  At least one limit or --legacy-events is required. prune covers every workspace in the selected store and skips
  running checkpoints. Completed captures whose
  start time is strictly older than N*24 hours are removed first; if remaining capture-container file
  bytes exceed the limit, oldest captures are removed until within budget. Age pruning also removes
  old PIRAEVT1 records across the store. --legacy-events explicitly removes ignored pre-1.0 JSON
  ledgers; they are otherwise preserved. Prints removed and remaining capture-file counts/bytes. Deletion
  is immediate; use list or stats before pruning when the scope needs inspection.

EXAMPLE
  pira_ctx prune --max-age-days 30 --max-store-bytes 1073741824"#;

const FORGET: &str = r#"pira_ctx forget — remove one capture or bounded operational history

USAGE
  pira_ctx forget [--store-dir PATH] RESULT
  pira_ctx forget [--store-dir PATH] history [--scope current|workspace]

BEHAVIOR
  RESULT resolves using normal ID/prefix/filename/path rules. An explicit path bypasses store lookup
  and may identify a valid capture outside --store-dir. The target must pass capture structure and
  integrity verification before removal. Running captures are rejected. `history` removes only event
  records: current automatically detected thread by default, or every thread in the current workspace
  with explicit --scope workspace. Captures and ignored pre-1.0 JSON events are unaffected. Deletion is immediate and not
  transactional across filesystem failures. The removed path or event count is printed.

EXAMPLES
  pira_ctx forget 20260712-052432
  pira_ctx forget history
  pira_ctx forget history --scope workspace"#;

const VERSION: &str = r#"pira_ctx version — print the installed pira_ctx version

USAGE
  pira_ctx --version
  pira_ctx version

OUTPUT
  Prints `pira_ctx MAJOR.MINOR.PATCH` and exits 0."#;

pub fn canonical_topic(topic: &str) -> Option<&'static str> {
    Some(match topic {
        "auto" | "default" => "auto",
        "capture" | "summary" => "capture",
        "exact" => "exact",
        "check" => "check",
        "batch" => "batch",
        "search" => "search",
        "range" => "range",
        "raw" => "raw",
        "transform" => "transform",
        "exec" => "exec",
        "recap" => "recap",
        "history" => "history",
        "stats" => "stats",
        "command" => "command",
        "verify" => "verify",
        "list" => "list",
        "prune" => "prune",
        "forget" => "forget",
        "version" | "--version" | "-V" => "version",
        _ => return None,
    })
}

pub fn command(topic: &str) -> Option<&'static str> {
    Some(match canonical_topic(topic)? {
        "auto" => AUTO,
        "exact" => EXACT,
        "check" => CHECK,
        "capture" => CAPTURE,
        "batch" => BATCH,
        "search" => SEARCH,
        "range" => RANGE,
        "raw" => RAW,
        "transform" => TRANSFORM,
        "exec" => EXEC,
        "recap" => RECAP,
        "history" => HISTORY,
        "stats" => STATS,
        "command" => COMMAND,
        "verify" => VERIFY,
        "list" => LIST,
        "prune" => PRUNE,
        "forget" => FORGET,
        "version" => VERSION,
        _ => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_global_commands_have_detailed_help() {
        for topic in [
            "auto",
            "exact",
            "check",
            "capture",
            "batch",
            "search",
            "range",
            "transform",
            "exec",
            "raw",
            "recap",
            "history",
            "stats",
            "command",
            "verify",
            "list",
            "prune",
            "forget",
        ] {
            let text = command(topic).unwrap();
            assert!(text.contains("USAGE"), "missing usage for {topic}");
            assert!(text.len() < 3_500, "help too long for {topic}");
        }
        assert!(GLOBAL.len() < 4_096);
        assert!(GLOBAL.contains("PIRA_CTX_MAX_RETAINED_BYTES"));
        assert!(GLOBAL.contains("PIRA_CTX_MAX_INDEXED_LINES"));
        assert!(AUTO.contains("not stored as a capture"));
        assert!(AUTO.contains("separate\n  history event"));
        assert!(RECAP.contains("--limit accepts 0..20"));
        assert!(HISTORY.contains("2,000 events per thread and 8,000 per workspace"));
        assert!(RAW.contains("prefer search, a narrow range, transform, or exec"));
    }
}
