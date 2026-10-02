### `pira_nav`: Read-Only Repository Navigator

#### Rules

- Choose by need: text → `search`; declaration/key/heading name → `symbols`; file structure → `outline`; known source target → `show`. Use `map` only for topology. Search/symbols/map default to cwd.
- Start with default bounds. `symbols` includes bounded source for unique matches; do not automatically follow with `show`. Reuse verified paths, targets, and evidence; stop once all answer parts are supported. Increase only omission-reported bounds; broaden/repeat only for a named unresolved gap.
- Batch related same-scope search/symbols queries with `-e` (independent ranking/accounting); one regex per conceptual query. Batch independent targets in one same-operation command; mix show/semantic operations with `query`, in request order. Query is not search; use standalone show for source-only batches.
- Targets: `FILE`, `FILE:START-END`, `FILE::ITEM`, or freshness-checked `outline --selectors` output. Hierarchy uses `::`, indices `[N]`, arbitrary segments JSON-style brackets (`["a.b"]`); shell-quote metacharacters. Exact paths precede unique suffixes; ambiguity errors, canonical paths never fall back to legacy aliases. Build Markdown targets from the outline's ancestor/local-title hierarchy.
- Ranges are inclusive: positive indices are 1-based; negatives count from the content's end (`-1` last). Content means the file for inline ranges, the preceding resolved target for postfix ranges. Zero, invalid starts, and reversed ranges error; oversized ends clip.
- Show is exact by default; `--glance` is clipped, line-numbered orientation. Do not use it for exact source; preserve requested expression punctuation.
- Lexical matches do not establish semantic identity: use LSP semantic commands when identity matters; report missing LSP instead of substituting text matches. Semantic targets: qualified names, selectors, or one-based UTF-8-byte `FILE:LINE:COLUMN`; show also accepts positions.
- Let structural backends auto-select. Use `--native` only to require clean bundled parsing, `--lsp` only to override server discovery.
- Before dash-prefixed positional paths/targets, use `--`; query instead pairs each operation option with its target.
- Do not use nav for binary/non-UTF-8 data, multiline/PCRE-only matching, archives, broad ignored-tree overrides, or symlink traversal.

#### Forms and Options

```text
pira_nav search|symbols QUERY [PATH...] [OPTIONS]
pira_nav outline FILE... [OPTIONS]
pira_nav show|SEMANTIC TARGET... [OPTIONS]
pira_nav map [PATH...] [OPTIONS]
pira_nav query --OPERATION TARGET [--OPERATION TARGET]... [OPTIONS]
```

SEMANTIC includes `definition`, `references`, `callers`, `callees`, `hover`; OPERATION is show or semantic. Search defaults to case-sensitive literal; symbols to case-insensitive exact name/suffix, then substring fallback.


| Option                  | Commands                                                 | Effect/scope                                                                                                                                                                           |
| ----------------------- | -------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `-e QUERY` (repeatable) | search, symbols                                          | Independent queries replacing the positional query.                                                                                                                                    |
| `--regex`               | search, symbols                                          | Rust regex;`(?i)` ignores case.                                                                                                                                                        |
| `-i`                    | search                                                   | Ignore case.                                                                                                                                                                           |
| `-g GLOB` (repeatable)  | search, map                                              | Gitignore-style path filter;`!` excludes.                                                                                                                                              |
| `-C N`                  | search                                                   | Context lines on each side.                                                                                                                                                            |
| `--files-with-matches`  | search                                                   | Paths instead of snippets.                                                                                                                                                             |
| `--max-depth N`         | map                                                      | Traversal depth; 0 visits specified paths only.                                                                                                                                        |
| `--range START:END`     | show, query-show                                         | Slice preceding target.                                                                                                                                                                |
| `--limit N`             | search; symbols; outline; map; non-hover semantics/query | Snippet lines/query (search: incompatible with`--files-with-matches` and `--count`); symbol rows/query; items across files; representative file rows; semantic rows/target or request. |
| `--max-items N`         | search                                                   | Total displayed lines/file rows; use with`--files-with-matches` or `--count` instead of `--limit`.                                                                                     |
| `--max-bytes N`         | search, show; hover, query                               | Shared source-block budget for search/show; per hover/query-show request. Shared caps may further limit search; oversized show blocks are omitted, not truncated.                      |

No search matches succeeds. Query options with no applicable operation error.

#### Examples

- Item's last line: `pira_nav show src/foo.rs::Foo::bar --range -1:-1`.
- Mixed batch: `pira_nav query --show src/foo.py::bar --references src/foo.py::bar`.
