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
<summary>Behavior, backends, baseline relationship, security, and benchmarks</summary>

`pira_codenav` is a read-only code-inspection tool for broad-to-narrow agent exploration:

1. `map` gives a bounded repository or subsystem shape.
2. `find` searches parsed declarations when a name is known but its file is not.
3. `outline` gives declarations and ranges without bodies; `show` returns only selected exact source.
4. `imports`, `dependents`, and `deps` expose conservative file relationships without a build system.
5. `definition`, `implementation`, `type-definition`, `references`, `callers`, `callees`, and `hover` provide precise semantics through a caller-supplied language server.

It supports 23 languages: Python, Rust, Java, C, C++, CUDA, Bash, Go, JavaScript, TypeScript/TSX, C#, PowerShell, PHP, Kotlin, Lua, HCL/Terraform, R, Ruby, Swift, Scala, Dart, Elixir, and Julia. PIRA setup installs one native executable in `PATH`; all grammars are compiled in. Native navigation needs no language runtime, daemon, database, network, project initialization, package manager, or runtime grammar download. Run `pira_codenav --help` and `pira_codenav SUBCOMMAND --help` for usage.

Output is compact and adaptive: predictable success defaults are omitted, while LSP use, failures, incomplete processing, omissions, truncation, and ambiguous or unsupported files remain explicit.

### Native and LSP backends

`imports`, `dependents`, `deps`, and `languages` are explicitly LSP-independent. `map`, `find`, `outline`, and structural `show` use Tree-sitter when the complete tree contains no `ERROR` or `MISSING` node. A structurally dirty file requires a suitable LSP document-symbol provider; PIRA does not label heuristic native recovery as clean.

Semantic commands require one-based `FILE:LINE:COLUMN` positions, where the column is a UTF-8 byte offset. They never fall back to textual guesses. Up to 32 targets in one invocation reuse the matching server. Optional bounded JSON files can supply initialization options and workspace settings through `--lsp-init` and `--lsp-settings`; language-qualified forms support mixed configurations.

Servers start lazily, are reused within one invocation, and are then shut down. Clean structural files do not launch a configured server. PIRA keeps no daemon or persistent index. Repository `map` and `find` parse in deterministic 16-file batches, bounding retained full-source memory while preserving order and LSP reuse.

Batch and repository commands retain useful successes. `complete=0` and bounded errors expose processing gaps; explicit item/byte omissions remain separate. If every attempted file or target fails, PIRA flushes bounded evidence and returns the underlying failure class.

### Relationship to ast-outline and Grove

ast-outline and Grove are useful functional baselines for compact structure and broad-to-narrow retrieval. PIRA keeps those ideas while avoiding runtime/project initialization and persistent state, and adds conservative file relationships plus a standard optional-LSP path. The comparison includes only overlapping clean tasks with task-specific correctness assertions. Unsupported or empty operations are omitted rather than counted as fast results; output schemas and exact retrieval semantics differ.

### Read-only and security boundary

- Repository code is parsed but never executed or edited. Ignore rules are honored, symlinked directories are not followed, and dependency targets outside the selected root are blocked.
- Exact source and LSP hover are framed as untrusted data. Unsafe terminal controls are escaped in source, hover, paths, symbols, signatures, dependency/call metadata, and errors.
- PIRA rejects `workspace/applyEdit`. A supplied server remains an external executable and may maintain its own caches; trust and configure it as an IDE server.
- Source, syntax depth, regex compilation, LSP messages/headers, configuration files, symbols, locations, call relations/ranges, hover, stderr, and reported errors are bounded. PIRA imposes no command timeout; the caller controls cancellation.

### Validation

| Property | Result |
|---|---:|
| Supported languages | 23 |
| Public correctness files | 74 |
| Clean native / correctly LSP-required files | 67 / 7 |
| Native structural targets | 1,047 |
| Location / freshness-selector round trips | 1,047/1,047 each |
| Curated essential-target recall | 72/72 |
| Functional / inert security / Rust tests | 78 / 16 / 14 |
| Reproducible benchmark tasks | 38 |

The retained Linux arm64 sandbox validates clangd 21.1.8 for definitions and incoming/outgoing call hierarchy, and basedpyright 1.39.9 for definition, implementation, type-definition, references, and hover. Deterministic fake-server tests additionally cover multi-target process reuse, initialization/settings forwarding, call-site normalization, independent capabilities, UTF-16 positions, rejected edits, malformed/oversized/hostile protocol data, lazy startup, and cached startup/parse failures.

### Performance

Each latency is a complete subprocess call through collected output. Native macOS arm64 measurements use 10 warmups and 100 calls. Same-sandbox Linux arm64 measurements use 5 warmups and 40 calls inside an already-running 2-CPU/4-GiB Docker Sandbox. Host and sandbox timings describe different environments; cross-tool comparisons use only same-sandbox columns.

| Clean operation | PIRA native | PIRA sandbox | ast-outline 1.8.2 sandbox | Grove 0.3.1 sandbox |
|---|---:|---:|---:|---:|
| Python outline | 5.512 ms | 2.236 ms | 51.213 ms | 41.027 ms |
| Rust outline | 7.036 ms | 3.562 ms | 53.623 ms | 44.535 ms |
| Python exact item | 5.557 ms | 2.322 ms | 54.374 ms | 42.160 ms |
| Python repository map | 4.806 ms | 1.026 ms | 51.170 ms | 37.437 ms |
| Rust repository map | 5.085 ms | 0.996 ms | 49.782 ms | 35.020 ms |
| Java outline | 4.103 ms | 1.006 ms | 48.960 ms | 29.933 ms |
| C outline | 3.269 ms | 0.410 ms | unsupported | 73.030 ms |
| C++ outline | 3.303 ms | 0.411 ms | 48.384 ms | 120.754 ms |

On these tasks, the fastest available baseline took 12.5–178× as long as PIRA in the same sandbox. PIRA used about 3.6–5.6 MiB peak RSS, versus about 15.6–48.4 MiB for the available baselines. The largest ratios use tiny synthetic C/C++ fixtures and principally measure complete-call overhead.

#### LSP cost

Each row is a cold complete call: PIRA starts and initializes the server, performs the request, then shuts it down. Measurements use 2 warmups and 15 calls in the same sandbox.

| Server and operation | Median | p95 |
|---|---:|---:|
| clangd definition | 158.495 ms | 162.704 ms |
| clangd references | 158.617 ms | 162.876 ms |
| clangd hover | 158.407 ms | 163.975 ms |
| clangd callers | 157.946 ms | 168.538 ms |
| clangd callees | 155.893 ms | 166.187 ms |
| basedpyright definition | 404.028 ms | 449.634 ms |
| basedpyright implementation | 560.360 ms | 604.304 ms |
| basedpyright type-definition | 566.938 ms | 613.717 ms |
| basedpyright references | 534.363 ms | 589.496 ms |
| basedpyright hover | 570.874 ms | 606.642 ms |

Real-server cost is dominated by server startup, project configuration, and caches. Batch semantic targets amortize that startup within one PIRA invocation.

With the deterministic protocol server, two definition targets took 33.056 ms median in one invocation versus 65.337 ms as two calls, a 1.98× speedup from shared startup and document state.

#### Subcommand context and latency

Context reduction compares returned UTF-8 bytes with complete source bytes otherwise needed by the fixed task. It is not a tokenizer-specific token count. Semantic rows use a deterministic minimal protocol server to isolate wrapper behavior rather than production-server initialization.

| Subcommand | Returned bytes | Context reduction | Native median | Sandbox median | Sandbox peak RSS |
|---|---:|---:|---:|---:|---:|
| `outline` | 1,652 | 92.4% | 5.512 ms | 2.236 ms | 3.6 MiB |
| `show` | 3,334 | 84.6% | 5.557 ms | 2.322 ms | 3.6 MiB |
| `map` | 432 | 69.9% | 4.806 ms | 1.026 ms | 4.6 MiB |
| `find` | 236 | 83.5% | 4.935 ms | 0.977 ms | 4.6 MiB |
| `imports` | 389 | 56.6% | 3.502 ms | 0.520 ms | 3.6 MiB |
| `dependents` | 228 | 84.1% | 4.684 ms | 0.954 ms | 4.6 MiB |
| `deps` | 342 | 76.1% | 4.771 ms | 0.927 ms | 4.1 MiB |
| `definition` | 147 | 99.3% | 32.831 ms | 18.338 ms | 12.7 MiB |
| `implementation` | 151 | 99.3% | 33.471 ms | 17.686 ms | 12.7 MiB |
| `type-definition` | 152 | 99.3% | 33.226 ms | 17.891 ms | 12.2 MiB |
| `references` | 1,435 | 93.4% | 33.399 ms | 18.100 ms | 12.7 MiB |
| `callers` | 237 | 98.9% | 33.152 ms | 17.170 ms | 12.7 MiB |
| `callees` | 237 | 98.9% | 32.557 ms | 17.745 ms | 12.7 MiB |
| `hover` | 200 | 99.1% | 33.123 ms | 17.642 ms | 12.7 MiB |
| `languages` | 166 | not applicable | 3.146 ms | 0.313 ms | 2.1 MiB |

`outline`, `show`, and semantic rows use the complete 21,680-byte pinned Click file. `find`, `map`, `dependents`, and `deps` use all 1,433 supported source bytes in the deterministic Python repository; `imports` uses its 896-byte file.

#### All-language outline regression

| Language | Fixture | Source | Outline | Reduction | Native | Sandbox |
|---|---|---:|---:|---:|---:|---:|
| Python | Click | 21,680 B | 1,652 B | 92.4% | 5.512 ms | 2.236 ms |
| Rust | ripgrep | 32,269 B | 2,884 B | 91.1% | 7.036 ms | 3.562 ms |
| Java | JUnit | 12,572 B | 1,371 B | 89.1% | 4.103 ms | 1.006 ms |
| C | synthetic | 210 B | 124 B | 41.0% | 3.269 ms | 0.410 ms |
| C++ | synthetic | 202 B | 272 B | −34.7% | 3.303 ms | 0.411 ms |
| CUDA | synthetic | 419 B | 198 B | 52.7% | 4.024 ms | 0.440 ms |
| Bash | bats-core | 16,510 B | 291 B | 98.2% | 5.022 ms | 2.793 ms |
| Go | synthetic | 119 B | 99 B | 16.8% | 3.559 ms | 0.274 ms |
| JavaScript | synthetic | 181 B | 211 B | −16.6% | 3.318 ms | 0.281 ms |
| TypeScript | synthetic | 386 B | 214 B | 44.6% | 3.371 ms | 0.404 ms |
| C# | synthetic | 235 B | 239 B | −1.7% | 3.412 ms | 0.413 ms |
| PowerShell | PowerShell | 17,197 B | 1,519 B | 91.2% | 5.395 ms | 10.884 ms |
| PHP | Laravel | 55,672 B | 7,444 B | 86.6% | 8.894 ms | 5.175 ms |
| Kotlin | kotlinx.coroutines | 17,043 B | 795 B | 95.3% | 3.858 ms | 1.043 ms |
| Lua | Neovim | 53,653 B | 2,071 B | 96.1% | 8.318 ms | 4.448 ms |
| HCL | Terraform | 2,248 B | 2,006 B | 10.8% | 3.645 ms | 0.591 ms |
| R | dplyr | 15,796 B | 712 B | 95.5% | 4.970 ms | 1.656 ms |
| Ruby | Rails | 14,867 B | 142 B | 99.0% | 3.905 ms | 1.471 ms |
| Swift | ArgumentParser | 10,088 B | 1,233 B | 87.8% | 4.459 ms | 5.402 ms |
| Scala | cats-effect | 82,458 B | 13,913 B | 83.1% | 27.503 ms | 22.177 ms |
| Dart | http | 3,856 B | 550 B | 85.7% | 4.126 ms | 1.080 ms |
| Elixir | Elixir | 42,726 B | 3,237 B | 92.4% | 8.855 ms | 9.027 ms |
| Julia | HTTP.jl | 4,675 B | 131 B | 97.2% | 3.939 ms | 1.065 ms |

Negative reduction means fixed structural metadata exceeds a tiny source fixture; it does not indicate lost source. This table is a parser-path regression check, not a repository-scale compression claim.

A minimal `pira_codenav --version` call measured 2.429 ms median / 2.686 ms p95 on native macOS. The optimized macOS arm64 binary is 51,190,960 bytes, or 5,709,612 bytes with deterministic gzip level 9.

<details>
<summary>Benchmark method and limitations</summary>

Pinned real fixtures are stored unmodified with adjacent licenses, immutable upstream commits, and SHA-256 records in `tests/resources/pira_codenav/SOURCES.md`; synthetic fixtures exercise compact language-specific constructs. Fixture source is parsed and read but never executed. Every timed task has a task-specific output assertion, so empty or incorrect output fails.

Baseline operations are functionally similar, not identical. Grove runtime grammar provisioning is excluded from repeated-call latency. LSP measurements depend on server, project size, initialization, flags, filesystem, and server caches. Real-server validation demonstrates protocol interoperability, not universal result completeness; the deterministic server validates PIRA protocol, bounds, and context behavior rather than production latency.

Measurements come from one Apple M1 Pro/macOS host and one Linux arm64 Docker Sandbox on that host. Complete-call latency is the product metric but does not isolate grammar throughput. Non-UTF-8 input, malformed syntax, path/symlink escapes, stale selectors, controls, closed output consumers, and hostile LSP messages are covered by functional/security tests rather than these tables. Reproducible runners are under `tests/tools`.

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
- `MEMORY_SYSTEM.md` — three-layer workspace-memory policy
- `USER.md` — user-specific knowledge and working preferences; keep this private
- `modules/` — optional task-specific modules for research, coding, writing, learning, guidance, and maintenance
- `assets/scripts/` — setup and helper scripts
- `tools/crates/` and `tools/Cargo.toml` — isolated Rust packages in the shared PIRA tools workspace
- `tools/build/build_pira_ctx_platform_bins.py` — shared pinned, reproducibility-checking release builder configured for `pira_ctx`
- `tools/build/build_pira_codenav_platform_bins.py` — package-isolated release entry point for `pira_codenav`
- `tools/src/pira_ctx/`, `tools/src/pira_decision/`, and `tools/src/pira_codenav/` — public Rust implementations
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
