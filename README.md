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

Compact per-workspace command events retain agent-supplied intents without duplicating command output. Intent history can be searched by case-insensitive text or Rust regex over an explicitly bounded newest-event window, with independently bounded newest-first results.

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
| Continuity | Bounded same-session recap after compaction | Explicit session lifecycle and continuation support |
| Safety scope | Preserves caller permissions; does not sandbox children | Adds sandbox and permission-policy integration |

PIRA uses `pira_ctx` when a small single-binary wrapper and exact local fallback are preferable. Context Mode is the more comprehensive option when broader interception, hooks, sandboxing, or database-backed retrieval are needed.

### Comprehensive held-out benchmark

The fixed benchmark caps each category at five cases and contains **45 sanitized responses across ten categories**. Its individual fixture contents were not seen during development of the output-selection design and were not used to tune selection, scoring, thresholds, injection heuristics, or live checkpointing; the fixed runner served as a regression and final measurement gate. The table reports `pira_ctx 0.8.0` on that corpus:

| Suite | Cases | Holdout source |
|---|---:|---|
| Public-repository core | 25 | New outputs generated after the freeze from ten fixed Rust repositories |
| Remote status workloads | 15 | Previously unseen Codex outputs streamed from a remote machine after the freeze |
| arXiv LaTeX supplement | 5 | Isolated builds of seeded recent arXiv papers, including natural and controlled failures |

The remote importer scanned raw logs in memory and persisted only fixed-point sanitized, privacy-audited fixtures; unsanitized server output was not written locally. Final selection is independent of PIRA output: SHA-256 order with a five-case cap, while build and test categories prefer three successes and two failures. No output routing, scoring, threshold, or security behavior changed after the final reported replay.

| Mode on the same 2,248,456 raw bytes | Returned context | Complete stored state | Median overhead | Immediate labeled evidence |
|---|---:|---:|---:|---:|
| `pira_ctx 0.8.0` automatic synopsis | 44,222 B (98.0% reduction) | 602,349 B (73.2% reduction) | +14.3 ms | 5/13 |
| Context Mode generic passthrough | 71,621 B (96.8% reduction) | 17,039,820 B (657.8% overhead) | +16.1 ms | 9/13 |
| `pira_ctx 0.8.0 check` | 3,064 B (99.9% reduction) | 602,484 B (73.2% reduction) | +13.2 ms | N/A—status only |
| Context Mode `ctx_index` receipt | 7,843 B (99.7% reduction) | 13,992,387 B (522.3% overhead) | N/A—no corresponding raw baseline | 0/13 |

All 45 PIRA cases preserved child status, entered full automatic-summary mode, reconstructed every sanitized output exactly, and passed integrity verification. Suggestions correctly abstained in 32/32 successful unlabeled cases; immediate evidence covered 5/8 failure markers and 0/5 changed basenames. Version 0.8.0 does not change selection or scoring: the same fixed replay gives identical quality counts with 0.7.1. Context Mode generic passthrough classified all 45 recorded statuses correctly and immediately exposed 7/8 failure markers plus 2/5 changed basenames. These quality figures were not used for tuning.

<details>
<summary>Benchmark method, category results, Context Mode comparison, and limitations</summary>

#### Corpus and evaluation protocol

The prospective public core covers VCS patches, largest tracked Rust files, recursive declaration listings, 40-commit terminal logs, and GitHub pull-list responses. Exact and structural duplicates against earlier private corpora were excluded. Public changed basenames were preserved as sanitized metadata so suggestion labels remained observable. Five cases per category were selected by content SHA-256, producing 25 core cases.

The remote extension was fixed before inspecting output content. It reconstructed completed `exec_command` and `write_stdin` sessions from 2.73 GB of authorized Codex logs, streamed 683 category candidates through an in-memory sanitizer, retained 289 eligible unique responses, and selected cases by outcome, size bucket, session diversity, and content hash. The final five-case cap retained three successful and two failed builds, three successful and two failed tests, four setup/install responses, and one static-analysis response. The server contained no LaTeX response above the 2 KiB threshold.

LaTeX coverage therefore uses arXiv sources compiled inside an isolated Linux Docker Sandbox with TeX Live and shell escape disabled. Candidate papers came from a binary-seeded shuffle of the recent `cs.LG` API pool. Repeated transport interruptions caused the live recent-entry pool to drift, so the five already downloaded public identifiers were frozen before corpus persistence or PIRA evaluation. One paper compiled successfully; its fresh source also produced a controlled undefined-command failure. Three additional papers contributed natural compilation failures, yielding one pass and four failures. Raw paper sources were disposable and were not committed.

Each suite's output-quality labels were fixed during the original holdout evaluation and were not revised for 0.8.0. The visible aggregate performance figures come from the final no-tuning 0.8.0 replay of the selected 45 fixtures through one persistent automatic store and one persistent `check` store. Every call used an identical raw fixture-emitter baseline; overhead is `wrapped wall time - raw-operation wall time`, summarized by the per-case median. Stored state includes captures, indexes, and event history but excludes installed binaries and runtimes.

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

## `pira_codenav`: lightweight structural code navigation

<details>
<summary>How it works, language scope, baseline relationship, and benchmarks</summary>

`pira_codenav` is a read-only native code-inspection tool for agents. It separates broad structural discovery from exact source reading so an agent can inspect a repository without repeatedly loading whole files:

1. `map` returns a bounded, mixed-language repository shape.
2. `outline` returns declarations and source ranges without implementation bodies; `--match` narrows a file locally.
3. `show` retrieves exact source for selected items or bounded line ranges.
4. `imports`, `dependents`, and `deps` expose conservative file-level relationships without invoking a build system.

`pira_codenav` supports Python, Rust, Java, C, C++, CUDA, Bash, Go, JavaScript, TypeScript/TSX, C#, PowerShell, PHP, Kotlin, Lua, HCL/Terraform, and R. PIRA setup installs it as one native executable in the user's `PATH`. Its seventeen Tree-sitter grammars are compiled in, so normal use requires no Python, language server, package manager, daemon, database, network, or runtime grammar download. Verified builds for macOS arm64/x64, Linux arm64/x64, and Windows x64 are bundled under `tools/dist/pira_codenav`.

### Read-only and semantic boundary

- Repository code and scripts are parsed but never executed. Directory traversal honors ignore rules, does not follow symlinks, and blocks structural dependency targets outside the workspace.
- Tree-sitter ranges and returned source are exact structural observations. When a bundled grammar lags current syntax, defective files may be reparsed through byte-aligned views: C/C++/CUDA recovery handles common macro and preprocessor scaffolding; Go, TypeScript, and C# recovery covers a small set of broadly specified newer syntax. Candidates must reduce navigation-relevant defects. Because C-family and C# outlines stop at recognized callable bodies, unsupported body-local syntax does not make declaration navigation incomplete. Results distinguish `ok`, `recovered`, and still-incomplete `partial` parses.
- Import targets marked `structural` are conservative path resolutions. Dynamic, external, ambiguous, build-dependent, and package-dependent targets remain visibly unresolved rather than being guessed.
- Exact source is framed as untrusted repository data and terminal control characters are escaped. The tool is not a sandbox and does not claim that source text is trustworthy instructions.
- Definitions, references, hover, type resolution, call hierarchy, diagnostics, rename, and refactoring remain language-server or compiler responsibilities. `pira_codenav` complements those systems rather than reimplementing them heuristically.

### Relationship to ast-outline and Grove

ast-outline and Grove are useful functional baselines for broad-to-narrow code reading. `pira_codenav` retains the useful ideas of compact outlines, repository maps, bounded exact retrieval, and stable item identities while choosing a smaller deployment boundary: one native executable, compile-time grammars, no project initialization, no persistent index, and no runtime grammar engine. It additionally exposes conservative file dependencies, but deliberately leaves semantic navigation to standard language servers.

The performance comparison below covers only operations that produced a semantically checked result. It is not a claim that outputs or feature sets are identical: PIRA location lookup returns one exact item, ast-outline name lookup can return overloads, Grove may return alternate IDs, and C++ parse completeness differs.

### Correctness benchmark

The public corpus combines synthetic edge cases with twelve pinned, unmodified real files and adjacent upstream licenses. Partial parses remain reported rather than counted as complete.

| Property | Result |
|---|---:|
| Supported languages | 17 |
| Files evaluated | 58 |
| Clean / recovered / partial parses | 51 / 6 / 1 |
| Emitted structural targets | 585 |
| Location-to-exact-item round trips | 585/585 |
| Freshness-selector round trips | 585/585 |
| Curated essential-target recall | 63/63 |

The recovered fixtures cover CUDA annotation macros, jq conditional entry points, fmt namespace-boundary macros, Go 1.26 value-initializing `new`, TypeScript variance and keyword tuple labels, and C# conditional constraints, ref expressions, extension blocks, and operators. `show` still returns untouched source. The sole partial file is intentionally malformed Python. All six newly added real-language fixtures parse completely. These results establish exact, self-consistent retrieval and curated recall on the public corpus, not universal language-semantic completeness.

### Complete-call latency

Every latency below measures one complete subprocess call—from immediately before process launch until output has been collected—and requires a task-specific semantic token in the response. This is the user-visible product metric, including executable startup, argument parsing, file access, parsing, and rendering. It is not an in-process parser-throughput measurement.

The cross-tool columns were measured together inside the already-running Linux arm64 Docker Sandbox with 2 CPUs and 4 GiB RAM, using 5 warmups and 40 measured calls. The native PIRA column was measured separately on an Apple M1 Pro running macOS arm64, using 10 warmups and 100 measured calls.

| Operation | `pira_codenav` native | `pira_codenav` sandbox | ast-outline 1.8.2 sandbox | Grove 0.3.1 sandbox |
|---|---:|---:|---:|---:|
| Python outline | 6.122 ms | 2.234 ms | 52.868 ms | 41.559 ms |
| Rust outline | 8.076 ms | 3.894 ms | 55.490 ms | 43.172 ms |
| Python exact item | 6.302 ms | 2.347 ms | 52.096 ms | 42.069 ms |
| Python repository map | 4.911 ms | 0.996 ms | 51.738 ms | 37.758 ms |
| Rust repository map | 4.970 ms | 0.964 ms | 51.048 ms | 34.562 ms |
| Java outline | 4.628 ms | 1.089 ms | 55.466 ms | 32.431 ms |
| C outline | 7.496 ms | 5.822 ms | unsupported | 91.818 ms |
| C++ outline | 4.127 ms | 1.247 ms | 51.452 ms | 125.005 ms |

Only the same-sandbox columns support cross-tool speedups: there, `pira_codenav` is 11–41x faster than the fastest available baseline and uses approximately 3.6–5.1 MiB peak RSS, compared with 23.9–25.9 MiB for ast-outline and 16.1–49.1 MiB for Grove. Native macOS values describe the expected direct deployment path but are not used to calculate those speedups.

Every public subcommand is also benchmarked directly. Each warmup and measured call must contain a task-specific semantic marker; a missing or failed result aborts the run. Native measurements use 10 warmups and 100 calls, while the already-running Linux sandbox uses 5 warmups and 40 calls.

| Subcommand | Returned bytes | Context reduction | Native median | Sandbox median | Sandbox peak RSS |
|---|---:|---:|---:|---:|---:|
| `outline` | 1,687 | 92.2% | 6.690 ms | 2.263 ms | 3.6 MiB |
| `show` | 3,380 | 84.4% | 6.121 ms | 2.276 ms | 3.6 MiB |
| `map` | 551 | 61.5% | 4.629 ms | 0.793 ms | 4.6 MiB |
| `imports` | 405 | 54.8% | 3.429 ms | 0.445 ms | 3.1 MiB |
| `dependents` | 234 | 83.7% | 4.619 ms | 0.814 ms | 4.1 MiB |
| `deps` | 385 | 73.1% | 4.492 ms | 0.843 ms | 4.6 MiB |
| `languages` | 199 | not applicable | 3.351 ms | 0.250 ms | 2.1 MiB |

`outline` and `show` use the 21,680-byte pinned Click fixture as their full-file baseline. `imports` uses the 896-byte input file; `map`, `dependents`, and `deps` use all 1,433 bytes of supported source scanned in the deterministic synthetic Python repository. These small fixtures exercise complete command behavior rather than repository scaling, so their reductions should not be compared directly with the large-repository table. `languages` has no source-byte baseline. The reproducible runner also records p95, minimum latency, stderr bytes, and exact commands.

The six added languages were measured separately on pinned files from important upstream repositories. Both environments used 10 warmups and 100 complete calls. Reduction compares UTF-8 outline bytes with source bytes; repository code was parsed but never executed.

| Language and pinned repository file | Source | Outline reduction | Native median | Sandbox median | Sandbox peak RSS |
|---|---:|---:|---:|---:|---:|
| PowerShell — PowerShell `tools/ResxGen/ResxGen.psm1` | 17,197 B | 90.9% | 5.502 ms | 9.940 ms | 4.1 MiB |
| PHP — Laravel `Collection.php` | 55,672 B | 86.6% | 9.225 ms | 4.668 ms | 5.1 MiB |
| Kotlin — kotlinx.coroutines `CoroutineDispatcher.kt` | 17,043 B | 95.1% | 3.593 ms | 0.963 ms | 6.1 MiB |
| Lua — Neovim `runtime/lua/vim/lsp.lua` | 53,653 B | 96.1% | 8.532 ms | 4.425 ms | 3.6 MiB |
| HCL — Terraform `apply-multi-var-comprehensive/root.tf` | 2,248 B | 9.3% | 3.406 ms | 0.576 ms | 3.1 MiB |
| R — dplyr `R/mutate.R` | 15,796 B | 95.3% | 4.996 ms | 1.643 ms | 3.6 MiB |

All six files returned the expected declarations. The HCL fixture is a compact, declaration-dense file: preserving every short block and attribute leaves little removable body text, so its low reduction reflects the input structure rather than truncation.

### Why native macOS is often slower than the sandbox

The sandbox result does not include a Docker boundary on every call. The benchmark driver itself runs inside the already-started container and directly launches the Linux binary; `sbx exec`, Docker startup, environment provisioning, and one-time dependency setup are outside the timed region. A fresh sandbox invocation for every query would add seconds and is neither the benchmark protocol nor intended use.

A separate minimal-call experiment isolates the fixed floor more clearly. After 20 warmups, `pira_codenav --version` was measured for 300 complete subprocess calls in each environment:

| Minimal complete call | Median | p95 |
|---|---:|---:|
| Native macOS arm64 | 3.396 ms | 4.175 ms |
| Already-running Linux arm64 sandbox | 0.229 ms | 0.334 ms |

The approximately 3.17 ms median fixed-cost gap explains most of the apparent native slowdown on operations that finish in only a few milliseconds. It does **not** show that parsing is generally faster in the sandbox. As a diagnostic only, subtracting the independently measured medians suggests that the startup gap fully explains the Kotlin and HCL totals and nearly all of the R difference; PHP and Lua retain modest native disadvantages, while PowerShell is substantially faster natively despite the startup penalty. Subtracting medians is not a rigorous parser benchmark, so these residuals are not reported as parser-throughput results.

The measurement establishes a different fixed call floor but does not isolate one operating-system component as the cause. Plausible contributors include macOS versus Linux process creation and executable loading, Mach-O/dyld versus ELF loader behavior, APFS versus Linux temporary storage, and warmed page-cache differences. The PowerShell reversal confirms that the sandbox CPU is not uniformly faster.

Complete-call latency remains the primary product metric because agents invoke a CLI, not an in-process parser library. Agents can amortize the fixed native cost by passing multiple files or targets to `outline`, `show`, and `imports`; repository `map` and dependency traversal already batch work internally. A persistent daemon could remove repeated launch cost but would conflict with the current lightweight, stateless deployment boundary. An in-process benchmark would characterize grammar throughput separately, but it would not replace the complete-call product results.

### Standard-repository scaling

Complete bounded maps were measured over pinned sparse public checkouts. Each row uses 3 warmups and 20 measured calls inside a Linux arm64 Docker Sandbox; reduction compares the complete map response with all supported source bytes in that checkout.

| Repository subset | Supported files | Clean / recovered / partial | Map reduction | Median | p95 | Peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| PyTorch `torch/nn` and selected ATen native/CUDA | 425 | 218 / 207 / 0 | 99.1% | 797.860 ms | 814.529 ms | 19.1 MiB |
| Go standard library `net/http`, `context`, and `io` | 195 | 194 / 1 / 0 | 98.5% | 202.939 ms | 210.103 ms | 14.1 MiB |
| TypeScript compiler | 77 | 75 / 2 / 0 | 99.8% | 433.184 ms | 454.241 ms | 74.6 MiB |
| .NET core `String`, `Span`, `List`, and tasks | 27 | 23 / 4 / 0 | 99.6% | 194.412 ms | 201.945 ms | 21.2 MiB |

Every eligible file produced a result without a hard failure or remaining partial parse. On the same PyTorch subset, the unnormalized parser reported 207 partial files; navigation-aware recovery reduced that incomplete set to zero while 401 files produced symbols. Recovery handles broad syntax classes including SFINAE value parameters, balanced preprocessor alternatives, namespace-boundary macros, dispatch macros with inline lambdas, Go value-initializing `new`, TypeScript variance/keyword tuple labels, and C# anti-constraints, conditional compilation, ref/unsafe expressions, C# 14 extension blocks, and null-conditional assignment. Views preserve byte and line offsets, reuse unchanged syntax trees incrementally, accept only structurally justified candidates, and never alter exact source returned by `show`. `recovered` means the supported declaration-navigation contract is complete: either a parser view resolved relevant syntax or remaining raw defects are confined to a recognized callable body that outline traversal intentionally does not inspect. It does not claim macro expansion, compiler-semantic correctness, or correctness of unsupported body syntax. TypeScript's peak memory reflects concurrent parsing of a few unusually large compiler files.

<details>
<summary>Benchmark method, task differences, and limitations</summary>

#### Cross-tool protocol

The benchmark environment contains the optimized Linux arm64 PIRA binary, ast-outline 1.8.2, and Grove 0.3.1 with the grammars needed by the fixed tasks. Unsupported operations and zero-definition no-ops are omitted rather than credited as fast results. Runtime grammar downloads and initialization required by Grove are setup costs and are excluded from repeated-call latency, as are one-time sandbox and tool provisioning costs.

The operations are closely matched but not identical. PIRA location lookup returns one exact item; ast-outline's Python name lookup can return multiple overloads; Grove may include alternate IDs. The baseline C++ parses are partial, and ast-outline's very small C++ response is not equivalent declaration coverage. Output size must therefore be interpreted together with useful target coverage and parse completeness.

#### New-language real-source protocol

Each added-language fixture is pinned to an immutable upstream commit, stored unmodified with its adjacent license, and SHA-256 recorded in `tests/resources/pira_codenav/SOURCES.md`. The baseline tools in that environment were not provisioned and semantically validated for these six grammars, so their rows are intentionally PIRA-only rather than presenting unsupported or incomparable baseline values.

#### Scaling protocol

The standard-repository rows use sparse, pinned checkouts rather than entire upstream repositories. `map --max-items 1000000` processes every inferred supported file in each checkout and returns explicit eligible, parsed, clean, recovered, partial, failed, shown, and omitted accounting. Repository code is read but never executed. The large TypeScript files and macro-heavy native sources are useful stress cases but do not represent every repository shape.

The four scaling corpora exposed recovery gaps during development, so these rows are not held-out generalization results. Each added syntax class was also checked after implementation on newly fetched, previously unseen source from an independent repository: MQT Core, NVIDIA CCCL, pytorch_scatter, Meta generative-recommenders, and Abseil for C-family recovery; golang/tools and Spacelift's Terraform provider for Go; typescript-eslint and the IMC Prosperity Visualizer for TypeScript; and Stryker.NET, PowerToys, Uno.Themes, Godot, and Microsoft.Unity.Analyzers for C#. Those repositories were parsed but never built or executed. Relative to the same no-recovery implementation (567.905 ms median and 18.1 MiB peak RSS), complete PyTorch recovery costs about 230 ms median and 1.0 MiB peak RSS on that macro-heavy 5.36 MB subset. Recovery work is gated to files that are initially defective, except that a structurally misparsed C# 14 extension block is recognized from the original tree before its aligned view is selected.

#### Limitations

These measurements come from one Apple M1 Pro/macOS host and one Linux arm64 Docker Sandbox on that machine; they are not universal hardware claims. Native and sandbox executables use different operating systems, executable formats, filesystems, and caches. The minimal `--version` experiment measures the fixed floor of a real CLI call, not pure kernel spawn time. Reductions are UTF-8 byte reductions rather than tokenizer-specific token counts. The curated corpus verifies structural extraction and exact identity round trips, not compiler semantics. Binary/non-UTF-8, malformed syntax, path escape, symlink, control-character, and stale-selector behavior are covered by functional and security tests rather than the latency tables.

</details>

The optimized seventeen-grammar macOS arm64 binary is 31.0 MB uncompressed and 3.49 MB with gzip level 9. Benchmark sources and reproducible runners are under `tests/resources/pira_codenav` and `tests/tools`.

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
