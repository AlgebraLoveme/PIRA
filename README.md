# PIRA — PI Research Assistant

PIRA (pronounced "Pyra") is a research-oriented personal agent for reasoning, writing, coding, learning, and practical problem-solving.

PIRA is designed to be warm, honest about uncertainty, evidence-first when evidence matters, and easy to inspect and customize.

## Tested compatibility

PIRA has been tested extensively with **Codex on GPT-5.4, GPT-5.5, and 5.6-sol, each with high reasoning effort**.

## Quick start

PIRA installs to `~/agent` by default. Setup is idempotent, backs up user-level Codex files before editing them, supports dry-run and verification modes, and is safe to rerun. Git is required; the setup wrapper handles Python discovery and can offer platform-specific installation help.

### Recommended one-line install or update

This command:
- uses the existing `~/agent` git checkout when present, otherwise clones PIRA into `~/agent`;
- enables **soft-safe** mode;
- keeps audio notifications **off**;
- links PIRA into Codex;
- installs or refreshes bundled PIRA tools such as `pira_ctx` in the user's `PATH`;
- moves old PIRA-managed legacy files into backup;
- creates a private `USER.md` placeholder only if `USER.md` is missing.

macOS/Linux:

```bash
if [ -d ~/agent/.git ]; then cd ~/agent && git pull --ff-only; else git clone https://github.com/AlgebraLoveme/PIRA.git ~/agent && cd ~/agent; fi && assets/scripts/setup_pira.sh --yes --execution-mode soft-safe --audio no --user-mode placeholder --global-agents link --legacy remove
```

Windows PowerShell:

```powershell
if (Test-Path "$HOME/agent/.git") { Set-Location "$HOME/agent"; git pull --ff-only; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE } } else { git clone https://github.com/AlgebraLoveme/PIRA.git "$HOME/agent"; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; Set-Location "$HOME/agent" }; powershell.exe -ExecutionPolicy Bypass -File assets/scripts/setup_pira.ps1 --yes --execution-mode soft-safe --audio no --user-mode placeholder --global-agents link --legacy remove
```

If you are rerunning setup and want a missing `USER.md` to stay missing, use `--user-mode keep` instead.

macOS/Linux:

```bash
cd ~/agent && git pull --ff-only && assets/scripts/setup_pira.sh --yes --execution-mode soft-safe --audio no --user-mode keep --global-agents link --legacy remove
```

Windows PowerShell:

```powershell
Set-Location "$HOME/agent"; git pull --ff-only; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; powershell.exe -ExecutionPolicy Bypass -File assets/scripts/setup_pira.ps1 --yes --execution-mode soft-safe --audio no --user-mode keep --global-agents link --legacy remove
```

`git pull --ff-only` updates an existing checkout only when Git can do so without a merge. If you have tracked local edits or a divergent branch, it stops for manual review.

> **Soft-safe is not a sandbox.** It sets Codex to no-approval/full-permission mode and relies on PIRA's explicit safety rules before state-changing commands.

### Inspect-first install

Use this path if you want to preview setup before writing anything:

```bash
git clone https://github.com/AlgebraLoveme/PIRA.git ~/agent
cd ~/agent
assets/scripts/setup_pira.sh --dry-run
assets/scripts/setup_pira.sh
assets/scripts/setup_pira.sh --verify
```

On Windows, invoke the same setup through `assets/scripts/setup_pira.ps1` from the repository directory:

```powershell
powershell.exe -ExecutionPolicy Bypass -File assets/scripts/setup_pira.ps1
```

Both platform wrappers forward the same options to `assets/scripts/setup_pira.py`. They share the Python bootstrap helpers in `assets/scripts/lib/`; setup can offer to install Python with Homebrew on macOS or winget on Windows.

## Setup options

<details>
<summary>Execution, user configuration, and tool-install options</summary>

The script asks before sensitive choices in interactive mode. For unattended setup, pass explicit flags.

### Execution mode

| Option | Codex settings | Use when |
| --- | --- | --- |
| `--execution-mode safe` | `approval_policy = "on-request"`, `sandbox_mode = "workspace-write"` | You want a real approval/sandbox boundary. |
| `--execution-mode soft-safe` | `approval_policy = "never"`, `sandbox_mode = "danger-full-access"` | You want convenience and accept full-permission risk. |
| `--execution-mode keep` | Leaves existing approval/sandbox settings unchanged. | You already manage Codex permissions yourself. |

### `USER.md` mode

| Option | Behavior |
| --- | --- |
| `--user-mode placeholder` | Creates a private placeholder `USER.md` when it is missing. Existing `USER.md` is preserved. |
| `--user-mode keep` | Leaves `USER.md` exactly as-is; if it is missing, setup leaves it missing. |
| `--user-mode interactive` | Asks what to do when `USER.md` is missing. |

### Other useful flags

| Option | Behavior |
| --- | --- |
| `--yes` | Accepts setup confirmations. It does **not** enable audio unless `--audio yes` is also set. |
| `--audio yes\|no\|ask` | Controls optional Codex audio notifications. Use `--audio no` for a quiet install. |
| `--global-agents link\|copy\|skip\|ask` | Controls whether `~/.codex/AGENTS.md` points to PIRA by symlink, copy, or not at all. |
| `--legacy remove\|keep\|ask` | Controls paths listed in `assets/LEGACY_LIST.md`; `remove` moves active legacy files into `.backup/setup_pira_legacy/`. |
| `--agent-dir PATH` | Installs against a path other than `~/agent`. |
| `--skip-tools` | Skips installation or refresh of bundled native PIRA tools. |
| `--tools-install-dir PATH` | Overrides the per-user tools directory (`~/.local/bin` on macOS/Linux or `%LOCALAPPDATA%\PIRA\bin` on Windows). |
| `--verify` | Checks the current setup without writing. |
| `--dry-run` | Prints planned changes without applying them. |

### Install or refresh only the PIRA tools

If PIRA is already configured and you only need to install, update, or reinstall its bundled native tools, run the tools-only setup from the existing PIRA checkout. Update that checkout first when you want a newer bundled release. The normal command installs a missing tool, replaces a stale copy, and leaves an identical verified copy unchanged.

On macOS or Linux:

```bash
cd ~/agent
python3 assets/scripts/setup_pira_tools.py          # install or refresh
python3 assets/scripts/setup_pira_tools.py --force  # reinstall the same bundled release
python3 assets/scripts/setup_pira_tools.py --verify # verify without writing
```

On Windows PowerShell:

```powershell
cd $HOME\agent
py -3 assets/scripts/setup_pira_tools.py          # install or refresh
py -3 assets/scripts/setup_pira_tools.py --force  # reinstall the same bundled release
py -3 assets/scripts/setup_pira_tools.py --verify # verify without writing
```

By default, the tools-only setup discovers every valid bundle under `tools/dist`, verifies all selected artifacts before writing, and installs each executable independently. Use `--tool pira_ctx` or `--tool pira_codenav` to operate on one tool; repeat `--tool` to select several. Use `--force` to reinstall even when an installed hash already matches, `--install-dir PATH` to override the default (`~/.local/bin` on macOS/Linux or `%LOCALAPPDATA%\PIRA\bin` on Windows), and `--no-path` when PATH persistence is managed separately. Restart the shell or agent process if setup reports that PATH changes are not yet active.

</details>

## What setup changes

<details>
<summary>Files, settings, tools, and verification performed by setup</summary>

The setup script:

1. Detects the repository directory and ensures it is available as `~/agent`, unless another `--agent-dir` is given.
2. Initializes a private `USER.md` placeholder when needed.
3. Moves legacy files listed in `assets/LEGACY_LIST.md` into `.backup/setup_pira_legacy/` when approved.
4. Updates or creates Codex `config.toml` so the selected agent directory's `AGENTS.md` is loaded, with `project_doc_max_bytes = 65536`.
5. Optionally links or copies `~/.codex/AGENTS.md` for Codex's global AGENTS discovery path.
6. Selects and verifies the bundled native tools for the current platform, then installs or refreshes them in a per-user PATH directory. Existing stale copies are atomically replaced; matching copies are left unchanged.
7. Optionally delegates audio setup to the platform-specific audio helper.
8. Verifies the setup, including the PIRA verification token and installed native tools.

If setup cannot safely handle an existing conflicting file or Codex setting, it stops or skips that action with a warning instead of silently overwriting it.

</details>

## Memory hierarchy

**Within its retention boundary, routed command history is not summarized away: events remain searchable.** PIRA stores operational history outside the prompt and retrieves it when needed, while keeping durable project understanding in a compact human-readable form.

| Level | Memory | Closest human analogue | Role |
|---|---|---|---|
| Project-global memory | `AGENT_WORKBOOK.md` | A project journal | Keeps durable actions, results, decisions, and lessons that should guide future work. |
| Event memory | `pira_ctx` storage | A detailed action log | Keeps command history and available evidence searchable without loading the full history into context. |
| Recovery view | `pira_ctx recap` | Memory of the last few steps | Restores the recent current-thread events needed after context compaction. |

<details>
<summary><strong>Difference from a Codex session log</strong></summary>

A Codex session log is **session-centered and chronological**: it preserves the conversation and activity of one thread so that the thread can be reviewed or continued. PIRA memory is **project-centered and retrieval-oriented**: it preserves useful operational evidence across threads and promotes durable understanding into the workspace workbook. It does not copy the conversation, hidden reasoning, or every transient interaction into a second transcript.

| Difference | Codex session log | PIRA memory hierarchy |
|---|---|---|
| Organizing unit | One conversation or thread | One workspace, with automatically scoped thread events |
| Primary content | Chronological conversation and session activity | Searchable command-purpose events and curated project knowledge |
| Retrieval model | Reopen or continue the session | Search relevant events, recap recent work, or read durable conclusions |
| Cross-session value | Preserves the history of each session | Carries project decisions and lessons into future sessions |
| Noise policy | Retains the conversational sequence | Keeps event detail in storage and promotes only lasting conclusions |

The two are complementary. A session log answers **“what happened in this conversation?”** PIRA answers **“what has been done in this project, what evidence remains available, and what should future work remember?”**

</details>

Memory moves upward by **promotion, not replacement**. Events remain in `pira_ctx` storage, and only conclusions with lasting value are promoted to the workbook. Agents therefore search detailed history instead of summarizing it away, and read global memory only when the task needs project continuity. Retention and explicit pruning still define how long event history remains available; anything that must outlive those bounds belongs in the workbook.

## `pira_ctx`: lightweight command context

<details>
<summary>How it works, security design, Context Mode comparison, and benchmarks</summary>

`pira_ctx` keeps large command output from overwhelming active context while retaining it locally within configurable space limits. Automatic mode is the default and its name can be omitted:

1. Ordinary short output is returned directly.
2. Long or diagnostic output is stored locally, while the model receives a bounded extractive synopsis and a capture ID.
3. If more detail is needed, the retained capture remains available for targeted search, line-range retrieval, transformation, or exact replay.

For compile, test, lint, or other validation jobs where only success or failure matters, `check` stores the retained log but prints one PASS/FAIL status line with the child exit code and capture ID.

Explicit `exact` mode streams unchanged when attached to a terminal. In non-interactive calls it buffers the result so that highly repetitive output—or output exceeding retention or indexing bounds—can switch to a retained report instead of flooding or silently truncating context. Genuinely varied output remains exact, and every switch is announced.

For a non-interactive program still running after about 30 seconds, `pira_ctx` silently publishes a read-only checkpoint. Concurrent inspection uses a consistent snapshot without blocking the program; `exec` receives a private copy. Running captures are protected from verification, deletion, and pruning until completion. See `pira_ctx --help` for the exact contract.

Compact command-purpose events retain agent-supplied intents without duplicating command output. `pira_ctx` automatically hashes `PIRA_CTX_THREAD_ID`, then `CODEX_THREAD_ID`, together with the workspace identity; raw thread identifiers are never stored. `pira_ctx history` reviews the current thread by default, while explicit `--scope workspace` merges anonymously labeled threads. Search covers all retained events by default but always stops at a bounded result limit of 10 unless changed. `--since`/`--until` select relative or RFC 3339 time bounds, while `--offset` and `--lookback` select event-number windows before intent matching—for example, the events before the newest 2,000. Literal intent matching is case-insensitive; fuzzy or semantic retrieval is not implicit, and regular expressions require `--regex`. With neither supported thread ID, current scope uses a labeled workspace-local unscoped fallback rather than claiming thread isolation.

Version 1.0 stores immutable checked `PIRAEVT1` records under a hidden `.events` namespace and builds disposable per-thread `PIRAEIX1` catalogs. The bounded checked retention journal also carries the history-facing summaries, letting a search scan one ordered index and validate only selected records instead of sorting and fingerprinting every event file. It rebuilds from authoritative records if missing or corrupt. Selected `history --details` rows additionally load duration and redacted command. Pre-1.0 JSON ledgers are ignored and preserved until explicit `prune --legacy-events` cleanup.

Setup installs a verified native executable in the user's `PATH`. Normal use requires no Python, Rust toolchain, daemon, database, network service, or model call; the optional `exec` command uses an available Python 3 interpreter to analyze a stored capture with explicit code. Captures are private user-cache files with independently compressed blocks and integrity hashes. `pira_ctx` preserves the caller's permissions and does not sandbox commands. Run `pira_ctx --help` to choose a command and `pira_ctx SUBCOMMAND --help` for exact usage. Its Rust source is under `tools/src/pira_ctx`, and verified builds for macOS arm64/x64, Linux arm64/x64, and Windows x64 are under `tools/dist/pira_ctx`.

### Security design

`pira_ctx` treats PROGRAM output as untrusted data, but it is not a sandbox and does not make the wrapped program safe. Its security boundary covers harm introduced by capturing, storing, selecting, or displaying PROGRAM output:

- **Injection-aware display.** Agent-facing extracts are labeled as PROGRAM data, which PIRA rules treat as untrusted, and use trusted line and stream prefixes. Terminal escapes, Unicode line separators, bidirectional overrides, and invisible direction controls are sanitized so output cannot forge report structure or manipulate normal automatic display. A bounded heuristic scans the final displayed text for reserved role/wrapper markers and common **English** injection keywords, including English instructions split across displayed lines. Keyword detection is not multilingual; non-English text is detected only when it also contains a recognized marker or unsafe display control. When triggered, one warning appears before the evidence. Detection never suppresses or re-ranks evidence, and benign output pays no warning-token cost.
- **Explicit exactness.** Automatic mode retains short output matched by the advisory heuristic instead of replaying it directly; short `exec` output follows the same routing. `search` applies the same warning. Exact byte-replay paths in `exact`, `raw`, and `range` remain unsanitized because utility and faithful recovery take precedence; they remain untrusted data under PIRA's agent rules.
- **Bounded space, unbounded time.** Retention defaults to 512 MiB and 1,000,000 indexed lines, with a 2,000,000-line hard ceiling, while eager Python `exec` materialization defaults to 64 MiB. These ceilings are configurable within their safety bounds. Excess output is drained but not retained, and the command continues. `pira_ctx` imposes no runtime timeout or time-based termination, leaving cancellation to the agent or user.
- **Private, checked storage.** Captures use private user-cache files, independently compressed and SHA-256-checked blocks, validated offsets and lengths, and authenticated metadata/index tables. Common secret-bearing command arguments are redacted from metadata, and result IDs do not derive from raw arguments. Output may still contain secrets, and integrity hashes detect corruption rather than authenticate data against a same-user attacker.

Security checks are separate from ordinary functional tests and run as fixed, non-destructive fixtures in a deny-by-default sandbox with deliberately tiny configurable limits. Against 0.7.1 on 45 held-out benign real logs, 0.8.0 produced no false warnings, returned byte-identical responses, and showed no measurable median runtime regression in an alternating comparison. The live concurrency contract was also exercised with an inert delayed program in an isolated Linux Docker Sandbox. This is best-effort hardening, not a guarantee that every adversarial instruction will be detected; the primary boundary is the rule that PROGRAM output is data and cannot grant authority.

### Relationship to Context Mode

`pira_ctx` was informed by [Context Mode](https://github.com/mksglu/context-mode), especially its ideas of keeping raw tool output out of context, attaching intent to execution, retrieving indexed evidence after compaction, and analyzing stored output with small programs. We thank its contributors for publishing and explaining these ideas.

| Dimension | `pira_ctx` | Context Mode |
|---|---|---|
| Integration | Native wrapper for explicit external commands | MCP server plus platform plugins and hooks |
| Runtime and storage | One Rust executable and self-contained checked capture files | Node/Bun integration with a SQLite FTS5 knowledge base |
| Reach | Commands deliberately routed through the wrapper | Broader shell, file, web, and MCP routing where integrations support it |
| Continuity | Bounded current-thread recap after compaction | Explicit session lifecycle and continuation support |
| Safety scope | Preserves caller permissions; does not sandbox children | Adds sandbox and permission-policy integration |

PIRA uses `pira_ctx` when a small single-binary wrapper and exact local fallback are preferable. Context Mode is the more comprehensive option when broader interception, hooks, sandboxing, or database-backed retrieval are needed.

### Comprehensive held-out benchmark

The fixed benchmark caps each category at five cases and contains **45 sanitized responses across ten categories**. Its individual fixture contents were not seen during development of the output-selection design and were not used to tune selection, scoring, thresholds, injection heuristics, or live checkpointing; the fixed runner served as a regression and final measurement gate. The table reports `pira_ctx 1.0.0` on that corpus:

| Suite | Cases | Holdout source |
|---|---:|---|
| Public-repository core | 25 | New outputs generated after the freeze from ten fixed Rust repositories |
| Remote status workloads | 15 | Previously unseen Codex outputs streamed from a remote machine after the freeze |
| arXiv LaTeX supplement | 5 | Isolated builds of seeded recent arXiv papers, including natural and controlled failures |

The remote importer scanned raw logs in memory and persisted only fixed-point sanitized, privacy-audited fixtures; unsanitized server output was not written locally. Final selection is independent of PIRA output: SHA-256 order with a five-case cap, while build and test categories prefer three successes and two failures. No output routing, scoring, threshold, or security behavior changed after the final reported replay.

| Mode on the same 2,248,456 raw bytes | Returned context | Complete stored state | Median overhead | Immediate labeled evidence |
|---|---:|---:|---:|---:|
| `pira_ctx 1.0.0` automatic synopsis | 44,222 B (98.0% reduction) | 615,047 B (72.6% reduction) | +24.4 ms | 5/13 |
| Context Mode generic passthrough | 71,621 B (96.8% reduction) | 17,039,820 B (657.8% overhead) | +16.1 ms | 9/13 |
| `pira_ctx 1.0.0 check` | 3,064 B (99.9% reduction) | 615,182 B (72.6% reduction) | +23.6 ms | N/A—status only |
| Context Mode `ctx_index` receipt | 7,843 B (99.7% reduction) | 13,992,387 B (522.3% overhead) | N/A—no corresponding raw baseline | 0/13 |

All 45 PIRA cases preserved child status, entered full automatic-summary mode, reconstructed every sanitized output exactly, and passed integrity verification. Suggestions correctly abstained in 32/32 successful unlabeled cases; immediate evidence covered 5/8 failure markers and 0/5 changed basenames. Version 1.0.0 retains the same selection and quality counts as the earlier replay. Context Mode generic passthrough classified all 45 recorded statuses correctly and immediately exposed 7/8 failure markers plus 2/5 changed basenames. These quality figures were not used for tuning.

<details>
<summary>Benchmark method, category results, Context Mode comparison, and limitations</summary>

#### Corpus and evaluation protocol

The prospective public core covers VCS patches, largest tracked Rust files, recursive declaration listings, 40-commit terminal logs, and GitHub pull-list responses. Exact and structural duplicates against earlier private corpora were excluded. Public changed basenames were preserved as sanitized metadata so suggestion labels remained observable. Five cases per category were selected by content SHA-256, producing 25 core cases.

The remote extension was fixed before inspecting output content. It reconstructed completed `exec_command` and `write_stdin` sessions from 2.73 GB of authorized Codex logs, streamed 683 category candidates through an in-memory sanitizer, retained 289 eligible unique responses, and selected cases by outcome, size bucket, session diversity, and content hash. The final five-case cap retained three successful and two failed builds, three successful and two failed tests, four setup/install responses, and one static-analysis response. The server contained no LaTeX response above the 2 KiB threshold.

LaTeX coverage therefore uses arXiv sources compiled inside an isolated Linux Docker Sandbox with TeX Live and shell escape disabled. Candidate papers came from a binary-seeded shuffle of the recent `cs.LG` API pool. Repeated transport interruptions caused the live recent-entry pool to drift, so the five already downloaded public identifiers were frozen before corpus persistence or PIRA evaluation. One paper compiled successfully; its fresh source also produced a controlled undefined-command failure. Three additional papers contributed natural compilation failures, yielding one pass and four failures. Raw paper sources were disposable and were not committed.

Each suite's output-quality labels were fixed during the original holdout evaluation and were not revised for 1.0.0. The visible aggregate performance figures come from a 1.0.0 replay of the selected 45 fixtures through one persistent automatic store and one persistent `check` store. Every call used an identical raw fixture-emitter baseline; overhead is `wrapped wall time - raw-operation wall time`, summarized by the per-case median. Stored state includes captures, indexes, and event history but excludes installed binaries and runtimes.

| Held-out category | Cases | Outcomes | Immediate quality | Context reduction |
|---|---:|---:|---:|---:|
| File reads | 5 | 5 success | 5/5 abstentions | 99.2% |
| GitHub pull retrieval | 5 | 5 success | 5/5 abstentions | 99.4% |
| Search and listing | 5 | 5 success | 5/5 abstentions | 98.9% |
| Terminal logs | 5 | 5 success | 5/5 abstentions | 95.5% |
| Version-control diffs | 5 | 5 success | 0/5 changed basenames | 91.1% |
| Builds | 5 | 3 success, 2 failure | 3/3 abstentions; 0/2 markers | 75.4% |
| Test runs | 5 | 3 success, 2 failure | 3/3 abstentions; 2/2 markers | 87.9% |
| Setup and installation | 4 | 4 success | 4/4 abstentions | 78.5% |
| Static analysis | 1 | 1 success | 1/1 abstention | 92.5% |
| LaTeX compilation | 5 | 1 success, 4 failure | 1/1 abstention; 3/4 markers | 94.6% |

#### Context Mode comparison on the final corpus

Context Mode 1.0.169 was installed inside an isolated Linux Docker Sandbox and rerun without errors on the exact final 45 sanitized fixtures. Generic passthrough used one persistent server, `ctx_execute_file`, the same category-level intent as PIRA, and JavaScript that printed each fixture while preserving its recorded exit status. Its direct Node emitter produced the same bytes and exit status as its raw baseline, so Docker startup and server initialization are excluded from overhead. It returned 71,621 bytes, classified all 45 statuses correctly, and immediately exposed 9/13 labeled outcomes: 7/8 failure markers and 2/5 changed basenames.

`ctx_index` used a separate persistent server and returned 7,843 bytes of indexing receipts. It exposed none of the 13 labels immediately, while exact content remained available through later search. Indexing has no equivalent raw operation, so no synthetic latency overhead is reported. Both Context Mode storage figures include its SQLite FTS5 retrieval state after shutdown; installed packages are excluded.

Generic passthrough is the closest automatic wrapper-level comparison, not Context Mode's recommended workflow. Context Mode normally asks the model to run task-specific analysis code and return only the derived answer. Its [published benchmark](https://github.com/mksglu/context-mode/blob/main/BENCHMARK.md) reports 98% reduction for task-specific execution, 82% for exact index-plus-search retrieval, and 96% overall. Returned-context measurements here count UTF-8 bytes rather than tokenizer-specific tokens, and immediate visibility does not measure evidence recoverable by later search.

#### Limitations

This remains a private implementation benchmark on one arm64 macOS evaluation host, not a universal performance claim. The remote suite is genuinely unseen and post-freeze imported, but its logs predate the freeze and are therefore not prospective outputs. Setup/install and static-analysis coverage remains below the five-case cap because no more eligible unique remote responses were available. Failure markers measure visibility of broad outcome evidence rather than complete diagnostic usefulness. arXiv selection required baseline build availability and includes one intentionally mutated source. Privacy sanitation changes path separators in LaTeX logs. Binary, non-UTF-8, and interactive-terminal behavior are covered by functional tests rather than this corpus. Web-search returns remain excluded because Codex built-in web output is not directly captured by the local command wrapper.

</details>

</details>

## `pira_codenav`: lightweight code navigation

<details>
<summary>Behavior, native and LSP backends, baseline relationship, security, and benchmarks</summary>

`pira_codenav` is a read-only native code-inspection tool for agents. It supports a broad-to-narrow workflow without requiring an IDE:

1. `map` returns a bounded mixed-language repository shape.
2. `outline` returns declarations and ranges without implementation bodies; `--match` narrows a file locally.
3. `show` returns exact source for selected items or bounded line ranges.
4. `imports`, `dependents`, and `deps` expose conservative file-level relationships without invoking a build system.
5. `definition`, `references`, and `hover` expose precise semantic navigation through a caller-supplied language server.
6. `languages` reports installed language and command capabilities.

It supports Python, Rust, Java, C, C++, CUDA, Bash, Go, JavaScript, TypeScript/TSX, C#, PowerShell, PHP, Kotlin, Lua, HCL/Terraform, R, Ruby, Swift, Scala, Dart, Elixir, and Julia. PIRA setup installs one native executable in `PATH`. All 23 Tree-sitter grammars are compiled in, so native navigation needs no language runtime, daemon, database, network, package manager, or runtime grammar download. Run `pira_codenav --help` to choose an operation and `pira_codenav SUBCOMMAND --help` for exact syntax.

### Native and LSP backends

LSP-independent operations remain explicit. `imports`, `dependents`, `deps`, and `languages` never start a language server. `outline`, structural `show`, and `map` use Tree-sitter when the complete native tree has no `ERROR` or `MISSING` node. If a grammar cannot cleanly parse a file, that structural target fails unless a suitable LSP is configured; PIRA does not present heuristic recovery as a clean result.

A caller-installed server can provide clean document symbols for structurally difficult files:

```bash
pira_codenav outline src/file.cpp \
  --lsp cpp=/absolute/path/to/clangd \
  --lsp-arg cpp=--background-index=0 \
  --lsp-root .
```

`definition`, `references`, and `hover` are deliberately LSP-only. They require `FILE:LINE:COLUMN`, where line and UTF-8 byte column are one-based, and never fall back to text-search guesses:

```bash
pira_codenav definition src/file.cpp:42:17 \
  --lsp cpp=/absolute/path/to/clangd \
  --lsp-root .
```

Use `--lsp /absolute/path` for one default server, or repeat `--lsp LANGUAGE=/absolute/path` and `--lsp-arg LANGUAGE=ARG` for mixed-language structural operations. Servers start lazily, are reused by language within one invocation, and are then shut down. PIRA maintains no daemon or persistent index. Clean structural files do not launch a configured server. `imports`, `dependents`, and `deps` require clean native syntax because standard LSP has no portable file-import graph.

Batch and repository commands preserve useful partial results. `complete=0` and bounded per-file errors identify processing gaps, while item/byte-limit omissions remain separate. If every attempted file or target fails, PIRA prints the bounded evidence and returns the underlying failure class. `dependents` and `deps` scan only the target language plus explicit C/C++/CUDA or JavaScript/TypeScript compatibility groups; `deps --direction both` alternates the two directions under its shared output bound.

The configured server owns semantic correctness. Results depend on the workspace root, project configuration, compiler flags, server implementation, and server-side caches. PIRA normalizes readable local LSP locations to its UTF-8 byte coordinates; locations it cannot safely normalize retain explicit LSP coordinates and encoding.

### Relationship to ast-outline and Grove

ast-outline and Grove are useful functional baselines for broad-to-narrow code reading. `pira_codenav` keeps compact outlines, bounded exact retrieval, stable freshness-checked item identities, and repository maps while using one native executable, compile-time grammars, no project initialization, and no persistent index. It adds conservative file relationships and a standard optional-LSP path for clean structural and semantic results.

The comparison below covers only overlapping clean tasks with task-specific output assertions. Outputs and feature sets are not identical: PIRA location lookup returns one exact item, ast-outline name lookup may return overloads, and Grove may expose alternate IDs. Unsupported or empty baseline operations are omitted rather than counted as fast results. Semantic timings are reported separately because the caller's language server dominates them and the baselines do not expose an equivalent interface in this benchmark.

### Read-only and security boundary

- Repository code and scripts are parsed but never executed. Traversal honors ignore rules, does not follow symlinks, and blocks structural dependency targets outside the workspace.
- Exact source and hover content are framed as untrusted data. Unsafe terminal controls are escaped in source, hover, paths, native/LSP symbols, dependency labels, and errors; per-target error count and size are bounded. Tool output is evidence, not instructions.
- PIRA rejects `workspace/applyEdit` and does not request edits. A supplied LSP is nevertheless an external executable and may create its own caches; trust and configure it as you would an IDE server.
- Source files, syntax-tree depth, LSP messages and headers, symbols, locations, hover output, and stderr capture are bounded. Pathologically deep trees are rejected before recursive extraction. PIRA does not impose an execution timeout.

### Validation

| Property | Result |
|---|---:|
| Supported languages | 23 |
| Public correctness files | 74 |
| Clean native Tree-sitter files | 67 |
| Files correctly rejected as LSP-required | 7 |
| Native structural targets | 1,047 |
| Location-to-item round trips | 1,047/1,047 |
| Freshness-selector round trips | 1,047/1,047 |
| Curated essential-target recall | 72/72 |
| Functional / inert security / Rust tests | 72 / 15 / 11 |

The Linux arm64 sandbox also validated real clangd 21.1.8 and basedpyright 1.39.9 for document symbols, definitions, references, and hover. Fake-server tests cover hierarchical and flat symbols, independent capability negotiation, UTF-16 positions, server-request refusal, malformed and oversized responses, bounded hostile diagnostics, hostile metadata and hover text, range escapes, lazy startup, failed-start and failed-parse reuse, and per-language process reuse.

### Performance

Each latency is a complete subprocess call from launch through collected output. The native column is direct macOS arm64 execution with 10 warmups and 100 measured calls. Same-sandbox columns were measured together inside an already-running Linux arm64 Docker Sandbox with 2 CPUs and 4 GiB RAM, using 5 warmups and 40 calls. Host and sandbox timings describe different environments and should not be compared as parser throughput; cross-tool comparisons use only same-sandbox columns.

| Clean operation | PIRA native | PIRA sandbox | ast-outline 1.8.2 sandbox | Grove 0.3.1 sandbox |
|---|---:|---:|---:|---:|
| Python outline | 5.601 ms | 2.048 ms | 48.337 ms | 39.423 ms |
| Rust outline | 7.176 ms | 3.269 ms | 50.499 ms | 40.287 ms |
| Python exact item | 5.556 ms | 2.038 ms | 49.816 ms | 40.095 ms |
| Python repository map | 4.667 ms | 0.774 ms | 47.958 ms | 35.686 ms |
| Rust repository map | 4.645 ms | 0.827 ms | 47.526 ms | 32.514 ms |
| Java outline | 4.293 ms | 0.929 ms | 45.753 ms | 27.930 ms |
| C outline | 3.312 ms | 0.324 ms | unsupported | 69.441 ms |
| C++ outline | 3.479 ms | 0.334 ms | 43.856 ms | 114.327 ms |

On these clean tasks, the fastest available baseline took 12.3–214× as long as PIRA in the same sandbox. PIRA used about 3.1–5.1 MiB peak RSS, versus about 15.6–46.5 MiB for ast-outline/Grove on overlapping rows. The largest ratios use tiny synthetic C/C++ fixtures, so they principally measure complete-call overhead.

#### LSP cost

Every row below is a cold complete call: PIRA starts and initializes the configured server, performs one request, and shuts it down. Real-server measurements use 2 warmups and 15 calls in the same sandbox.

| Server and operation | Median | p95 |
|---|---:|---:|
| clangd definition | 173.082 ms | 178.642 ms |
| clangd references | 173.880 ms | 177.944 ms |
| clangd hover | 176.831 ms | 180.309 ms |
| basedpyright definition | 370.001 ms | 383.492 ms |
| basedpyright references | 488.983 ms | 526.721 ms |
| basedpyright hover | 516.557 ms | 540.636 ms |

A server is reused for all matching files within one structural invocation but not across separate CLI calls; the server may retain its own external cache. A configured server adds no process-start cost to a clean native structural result.

#### Subcommand context and latency

Context reduction compares returned UTF-8 bytes with the complete source bytes otherwise needed by the fixed task, not tokenizer-specific token counts. Semantic rows use a deterministic protocol fixture so the measurements isolate PIRA plus a minimal server process rather than a production server's initialization cost.

| Subcommand | Returned bytes | Context reduction | Native median | Sandbox median | Sandbox peak RSS |
|---|---:|---:|---:|---:|---:|
| `outline` | 1,698 | 92.2% | 5.601 ms | 2.025 ms | 3.6 MiB |
| `show` | 3,381 | 84.4% | 5.556 ms | 2.027 ms | 3.6 MiB |
| `map` | 631 | 56.0% | 4.667 ms | 0.768 ms | 4.6 MiB |
| `imports` | 405 | 54.8% | 3.483 ms | 0.413 ms | 3.6 MiB |
| `dependents` | 296 | 79.3% | 4.728 ms | 0.791 ms | 4.6 MiB |
| `deps` | 428 | 70.1% | 4.706 ms | 0.784 ms | 4.6 MiB |
| `definition` | 193 | 99.1% | 32.617 ms | 14.924 ms | 12.2 MiB |
| `references` | 1,463 | 93.3% | 34.088 ms | 15.275 ms | 12.2 MiB |
| `hover` | 276 | 98.7% | 33.726 ms | 14.901 ms | 12.2 MiB |
| `languages` | 280 | not applicable | 3.152 ms | 0.247 ms | 2.1 MiB |

`outline`, `show`, and the semantic rows use the complete 21,680-byte pinned Click file. `imports` uses its 896-byte input; `map`, `dependents`, and `deps` use all 1,433 supported source bytes scanned by the deterministic Python fixture.

#### All-language outline check

| Language | Fixture | Source | Outline | Reduction | Native | Sandbox |
|---|---|---:|---:|---:|---:|---:|
| Python | Click | 21,680 B | 1,698 B | 92.2% | 5.601 ms | 2.025 ms |
| Rust | ripgrep | 32,269 B | 2,928 B | 90.9% | 7.176 ms | 3.257 ms |
| Java | JUnit | 12,572 B | 1,415 B | 88.7% | 4.293 ms | 0.929 ms |
| C | synthetic | 210 B | 165 B | 21.4% | 3.312 ms | 0.320 ms |
| C++ | synthetic | 202 B | 315 B | −55.9% | 3.479 ms | 0.343 ms |
| CUDA | synthetic | 419 B | 242 B | 42.2% | 3.479 ms | 0.407 ms |
| Bash | bats-core | 16,510 B | 335 B | 98.0% | 5.821 ms | 2.606 ms |
| Go | synthetic | 119 B | 141 B | −18.5% | 3.351 ms | 0.276 ms |
| JavaScript | synthetic | 181 B | 261 B | −44.2% | 3.365 ms | 0.301 ms |
| TypeScript | synthetic | 386 B | 264 B | 31.6% | 3.528 ms | 0.333 ms |
| C# | synthetic | 235 B | 285 B | −21.3% | 3.360 ms | 0.361 ms |
| PowerShell | PowerShell | 17,197 B | 1,569 B | 90.9% | 5.687 ms | 9.890 ms |
| PHP | Laravel | 55,672 B | 7,487 B | 86.6% | 8.826 ms | 4.537 ms |
| Kotlin | kotlinx.coroutines | 17,043 B | 841 B | 95.1% | 3.903 ms | 0.973 ms |
| Lua | Neovim | 53,653 B | 2,114 B | 96.1% | 8.326 ms | 4.301 ms |
| HCL | Terraform | 2,248 B | 2,049 B | 8.9% | 3.692 ms | 0.570 ms |
| R | dplyr | 15,796 B | 753 B | 95.2% | 4.948 ms | 1.615 ms |
| Ruby | Rails | 14,867 B | 186 B | 98.7% | 3.679 ms | 1.079 ms |
| Swift | ArgumentParser | 10,088 B | 1,278 B | 87.3% | 4.256 ms | 4.807 ms |
| Scala | cats-effect | 82,458 B | 13,958 B | 83.1% | 27.169 ms | 21.322 ms |
| Dart | http | 3,856 B | 594 B | 84.6% | 3.946 ms | 1.031 ms |
| Elixir | Elixir | 42,726 B | 3,283 B | 92.3% | 8.301 ms | 8.589 ms |
| Julia | HTTP.jl | 4,675 B | 176 B | 96.2% | 3.691 ms | 1.063 ms |

Negative reduction means fixed structural metadata is larger than a tiny source fixture; it does not indicate lost or duplicated source. HCL is similarly declaration-dense. The table is a language-path regression check, not a claim that synthetic files represent repository-scale compression.

A minimal `pira_codenav --version` complete call measured 3.274 ms median / 3.860 ms p95 on native macOS and 0.249 ms / 0.319 ms inside the already-running sandbox. The sandbox result excludes `sbx exec`, Docker startup, and provisioning. The optimized macOS arm64 binary is 51,124,208 bytes, or 5,687,626 bytes with deterministic gzip level 9.

<details>
<summary>Benchmark method and limitations</summary>

Pinned real fixtures are stored unmodified with adjacent licenses, immutable upstream commits, and SHA-256 records in `tests/resources/pira_codenav/SOURCES.md`; synthetic fixtures exercise compact language-specific constructs. Repository source is parsed and read but never executed. Every timed task must return a task-specific marker, so an empty or incorrect response fails.

The baseline operations are functionally similar, not identical. Grove runtime grammar provisioning is excluded from repeated-call latency. LSP timings depend strongly on server, project size, initialization, build configuration, filesystem, and server-side caches. Real-server validation demonstrates the protocol path, not universal symbol completeness. The deterministic semantic fixture validates PIRA's protocol and bounds but is not representative of production-server latency or memory.

Measurements come from one Apple M1 Pro/macOS host and one Linux arm64 Docker Sandbox on that host. Complete-call latency is the CLI product metric but does not isolate grammar throughput. Binary/non-UTF-8 input, malformed syntax, path and symlink escapes, control characters, stale selectors, output-consumer behavior, and hostile LSP messages are covered by functional/security tests rather than the latency tables. Reproducible runners are under `tests/tools`.

</details>

</details>

## Optional Codex audio notifications

<details>
<summary>Behavior, customization, and manual installation</summary>

Audio notifications are optional and are supported only for **Codex on macOS or Windows**. They are off by default and should not be presented as supported for Claude Code, other agent tools, Linux, or other systems.

When enabled, PIRA can play:
- `complete_msg.m4a` when the direct user-facing Codex agent finishes a turn; and
- `waiting_msg.m4a` when the direct user-facing Codex agent needs confirmation, approval, or another user action.

Startup audio is no longer installed. The helpers remove legacy PIRA-managed startup wrappers when found.

Focus detection is best-effort. On macOS, the helper checks the frontmost app with `osascript`; on Windows, it checks the foreground window process with built-in PowerShell/.NET calls. If a known terminal or editor is focused, including VS Code-like integrated-terminal hosts, the helper stays quiet. Subagent turns are suppressed by detecting Codex session metadata.

The default audio set lives in `~/agent/PIRA_Voice/Samantha`. A custom audio set is any folder containing:

```text
complete_msg.m4a
waiting_msg.m4a
```

For customization guidance, postprocessing steps, and ready-to-paste prompts for PIRA, see `~/agent/assets/AUDIO_CUSTOMIZATION_GUIDE.md`.

### Install audio manually

Prefer `assets/scripts/setup_pira.* --audio yes` when installing PIRA. If you only want to configure audio, use the dedicated helpers.

macOS:

```bash
bash ~/agent/assets/scripts/setup_codex_audio_mode.sh \
  --config ~/.codex/config.toml
```

Windows PowerShell:

```powershell
powershell.exe -ExecutionPolicy Bypass -File "$HOME\agent\assets\scripts\setup_codex_audio_mode_windows.ps1" `
  -ConfigPath "$HOME\.codex\config.toml"
```

Use `--audio-dir PATH` on macOS or `-AudioDir PATH` on Windows for a custom audio set. Restart Codex after installing or changing audio mode.

If `config.toml` already has a top-level `notify` entry, inspect it first and rerun the relevant helper with `--force` on macOS or `-Force` on Windows only after confirming it is acceptable to replace.

Keep `notify` at the top level of `config.toml`, before any `[section]` table, so it is not accidentally parsed as part of a nested table.

</details>

## What PIRA is for

PIRA is meant to help with work that benefits from both care and rigor:

- research planning and evidence-based analysis;
- scientific writing and paper polishing;
- coding, debugging, and repository work;
- learning and explanation;
- practical day-to-day guidance.

## Core principles

PIRA is built around a few simple commitments:

- **Be useful.** Prefer concrete next steps over vague advice.
- **Be honest.** Do not fabricate claims, citations, or results.
- **Be evidence-first.** Use primary sources when facts matter.
- **Be transparent.** Separate observation from interpretation and state uncertainty clearly.
- **Be kind.** Stay supportive, collaborative, and respectful.

## Why this design

PIRA is intentionally minimal:

- **Inspectable.** Behavior is organized in readable policy and module files that are easy to review and customize.
- **Lightweight.** Token overhead stays low; there is no heavy framework or rarely used abstraction layer.
- **Research-oriented.** Default workflows emphasize reasoning, writing, coding, evidence gathering, and careful iteration.
- **Lean by default.** Drawing on [Ponytail](https://github.com/DietrichGebert/ponytail) and general lessons from *Clean Code* and *Clean Architecture*, the coding style favors deletion, standard-library or platform features, the smallest safe implementation, readable names, and clear boundaries over speculative abstractions.
- **Tool-friendly.** The small, explicit design integrates naturally with official tools such as Codex.

## Safety model

<details>
<summary>Permission boundaries and operating rules</summary>

PIRA can run in soft-safe full-permission mode, but it is not a sandbox. Its safety depends on explicit operating rules in `TOOLS.md`, including:

- before any command that may write or change state, print a brief safety review covering action, scope, destructive risk, secrets/privacy impact, and rollback path when available;
- prefer narrow, reversible actions;
- avoid destructive commands without explicit permission;
- keep temporary artifacts in the platform temp directory unless the user wants them preserved.

Subagents should load the same bootstrap policy as the main agent. This is handled by Codex but has not been tested on other agents.

</details>

## Repository layout

<details>
<summary>Source, policy, setup, and bundled-tool files</summary>

- `AGENTS.md` — bootstrap instructions and module routing policy
- `SOUL.md` — PI's identity, tone, and non-negotiable behaviors
- `TOOLS.md` — tool-use and safety rules
- `USER.md` — user-specific knowledge and working preferences; keep this private
- `modules/` — optional task-specific modules for research, coding, writing, learning, guidance, and maintenance
- `assets/scripts/` — setup and helper scripts
- `tools/crates/` and `tools/Cargo.toml` — isolated Rust packages in the shared PIRA tools workspace
- `tools/build/build_pira_ctx_platform_bins.py` — shared pinned, reproducibility-checking release builder configured for `pira_ctx`
- `tools/build/build_pira_codenav_platform_bins.py` — package-isolated release entry point for `pira_codenav`
- `tools/src/pira_ctx/` and `tools/src/pira_codenav/` — public Rust implementations
- `tools/dist/pira_ctx/` and `tools/dist/pira_codenav/` — verified platform executables and per-tool bundle manifests
- `tests/tools/` and `tests/resources/pira_codenav/` — public codenav checks, benchmarks, pinned fixtures, provenance, and adjacent licenses
- `PIRA_Voice/Samantha/` — default audio clips for optional Codex notifications

</details>

## Public/private split

<details>
<summary>What belongs in the public repository and what stays local</summary>

The public repository contains the shared policy framework. Personal context should stay local:

- keep `USER.md` private;
- keep workspace-specific memory in local `AGENT_WORKBOOK.md` files;
- do not commit secrets or sensitive personal information.

</details>

## Why the name PIRA

PIRA stands for PI Research Assistant, giving PI a clear public-facing project name.

## Acknowledgement and citation

If PIRA materially assists a research project, disclose that assistance where appropriate, such as in an acknowledgement, LLM-use disclosure, or reproducibility checklist, and cite this repository. Adapt the scope of assistance to what was actually used, and include the actual model/version or reasoning setting if your venue asks for that level of detail.

Suggested disclosure text:

> This paper was assisted by PIRA~\citep{pira}, a research-assistant agent powered by {the model used, such as GPT-5.5}. The assistance included [brainstorming / implementation assistance / writing polish / ...]. The authors are fully responsible for the final content.

Suggested BibTeX entry:

```bibtex
@misc{pira,
  author = {{PIRA Project}},
  title = {{PIRA}: {PI} Research Assistant},
  year = {2026},
  howpublished = {\url{https://github.com/AlgebraLoveme/PIRA}}
}
```

PIRA should be acknowledged as tool assistance, not as scientific authorship.

## License

PIRA is available under the [Apache License 2.0](LICENSE).
