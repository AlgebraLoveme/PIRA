# PIRA — PI Research Assistant

PIRA (pronounced "Pyra") is a research-oriented personal agent for reasoning, writing, coding, learning, and practical problem-solving.

It adds a small, inspectable set of instructions and tools to Codex. The goal is simple: make the agent more careful, useful, and consistent without hiding how it works.

## What PIRA helps with

- planning research and checking evidence;
- writing and polishing scientific text;
- coding, debugging, and repository work;
- learning difficult material through clear explanations;
- practical day-to-day problem-solving.

PIRA follows five principles:

- **Useful:** give concrete next steps, not vague advice.
- **Honest:** never invent claims, citations, or results.
- **Evidence-first:** prefer primary sources when facts matter.
- **Transparent:** separate facts from interpretation and state uncertainty.
- **Kind:** stay supportive, collaborative, and respectful.

## Tested compatibility

PIRA has been tested extensively with **Codex on GPT-5.4, GPT-5.5, and 5.6-sol, each using high reasoning effort**. Other models or agent platforms may work, but have not received the same level of testing.

## Quick start

PIRA installs to `~/agent` by default. You can use the one-line command for the easiest setup, or the inspect-first path if you want to review every change before it happens.

Setup is safe to rerun: it preserves an existing `USER.md`, backs up user-level Codex files before changing them, and can preview or verify its work. Git is required. The setup helper checks for Python and can offer platform-specific installation help.

### Recommended one-line install or update

The recommended command installs or updates PIRA, then connects it to Codex. It:
- uses the existing `~/agent` git checkout when present, otherwise clones PIRA into `~/agent`;
- enables **soft-safe** mode;
- keeps audio notifications **off**;
- links PIRA into Codex;
- installs or refreshes bundled PIRA tools in the user's `PATH`;
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

If you are updating PIRA and intentionally do not use `USER.md`, choose `--user-mode keep`:

macOS/Linux:

```bash
cd ~/agent && git pull --ff-only && assets/scripts/setup_pira.sh --yes --execution-mode soft-safe --audio no --user-mode keep --global-agents link --legacy remove
```

Windows PowerShell:

```powershell
Set-Location "$HOME/agent"; git pull --ff-only; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; powershell.exe -ExecutionPolicy Bypass -File assets/scripts/setup_pira.ps1 --yes --execution-mode soft-safe --audio no --user-mode keep --global-agents link --legacy remove
```

`git pull --ff-only` refuses to create an automatic merge. If your checkout has conflicting local work, it stops so you can review it safely.

> **Soft-safe is not a sandbox.** It sets Codex to no-approval/full-permission mode and relies on PIRA's explicit safety rules before state-changing commands.

### Inspect-first install

Use this path if you prefer to inspect setup before it changes anything:

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

The macOS/Linux and Windows wrappers support the same options. If Python is missing, setup can offer to install it with Homebrew on macOS or winget on Windows.

## Setup options

<details>
<summary>Execution, user configuration, and tool-install options</summary>

Most users can keep the recommended defaults. Open this section when you want stricter permissions, a custom install path, audio, or a tools-only update. Interactive setup asks before sensitive choices; unattended setup requires explicit flags.

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

If PIRA itself is already configured, these commands update only its bundled tools. A normal run installs missing tools, replaces outdated copies, and leaves verified matching copies unchanged.

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

The updater verifies every selected bundle before installation. Use `--tool pira_ctx` or `--tool pira_codenav` to select one tool, and repeat `--tool` to select several. `--force` reinstalls an already matching copy; `--install-dir PATH` changes the destination; `--no-path` leaves PATH management to you. Restart the shell or agent process if setup says the new PATH is not active yet.

</details>

## What setup changes

<details>
<summary>Files, settings, tools, and verification performed by setup</summary>

In plain language, setup connects PIRA to Codex, installs its tools, and checks the result. More precisely, it:

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

## Memory System

PIRA separates memory by how long it should remain useful. This prevents raw command logs from crowding out important project knowledge.

| Layer | What it remembers | Where it lives | Who reads it |
|---|---|---|---|
| Activity | Commands, actions, and detailed evidence | `pira_ctx` | Agents search it when needed |
| Decisions | Important choices, alternatives, and reasons | `pira_decision` | Agents search it when needed |
| Project knowledge | Durable state, validated results, lessons, and limitations | `AGENT_WORKBOOK.md` | Agents and humans read it directly |

Detailed activity remains searchable for as long as retention allows. After a conversation is compacted, `pira_ctx recap` can restore the recent activity needed to continue the work; it is part of the activity layer, not another memory layer.

<details>
<summary><strong>Difference from a Codex session log</strong></summary>

A Codex session log is **session-centered and chronological**: it preserves the conversation and activity of one thread so that the thread can be reviewed or continued. The PIRA Memory System is **project-centered and retrieval-oriented**: it preserves useful operational evidence across threads and promotes durable understanding into the workspace workbook. It does not copy the conversation, hidden reasoning, or every transient interaction into a second transcript.

| Difference | Codex session log | PIRA Memory System |
|---|---|---|
| Organizing unit | One conversation or thread | One workspace, with automatically scoped thread events |
| Primary content | Chronological conversation and session activity | Command events, structured decisions, and curated high-level project knowledge |
| Retrieval model | Reopen or continue the session | Agents search lower layers; agents and humans read the workbook directly |
| Cross-session value | Preserves each session's history | Carries operational evidence, decisions, and durable project understanding across sessions |
| Noise policy | Retains the conversational sequence | Separates event detail, concluded decisions, and promoted high-level knowledge |

The two are complementary. A session log answers **“what happened in this conversation?”** PIRA answers **“what has been done in this project, what evidence remains available, and what should future work remember?”**

</details>

Information moves upward only when its lasting value increases. Activity stays in `pira_ctx`; concluded choices go to `pira_decision`; durable consequences become self-contained workbook entries. The workbook never requires a reader to look up a lower-level record.

## PIRA Internal Tools

PIRA includes three small tools that help agents work with less noise and better continuity. Most users never need to run them manually; built-in command help provides exact syntax.

| Tool | What problem it solves |
|---|---|
| `pira_ctx` | Keeps large command output available without flooding the active conversation. |
| `pira_decision` | Records important choices in a consistent, searchable form. |
| `pira_codenav` | Lets an agent inspect code structure and relationships without editing or executing the code. |

### Agent-level evaluation

An isolated development benchmark tested whether the tools help a fresh agent answer exact questions, not merely whether the binaries are fast in microbenchmarks.

<details>
<summary>Method, results, and interpretation</summary>

#### `pira_ctx`: natural implementation task

Two fresh `gpt-5.6-sol` high-reasoning agents received byte-identical prompts and clean Git repositories containing only the current `pira_decision` design. Both were asked to implement and test the complete tool in Python. No command output, capture, prescribed workflow, existing implementation, or answer was supplied. The tool condition had only `pira_ctx 1.4.0` and its ctx-specific rules; the baseline had no PIRA rules or binaries, and neither condition had codenav or a decision binary. Both could write their isolated workspace and use ordinary Python and Git.

Each generated implementation was then run inside its sandbox against the same 20 private black-box cases covering validation, JSON views, search, workspace scope, the checked record envelope, corruption, symlink rejection, safe deletion, concurrency, bounds, and temporary-file isolation.

| Correctness, tool / baseline | Total tokens, tool / baseline | Noncached tokens | Wall time | Command-output bytes | Commands, tool / baseline |
|---:|---:|---:|---:|---:|---:|
| 20/20 / 20/20 | 670,031 / 819,876 (**−18.3%**) | 70,735 / 108,964 (**−35.1%**) | 690.2 / 918.2 s (**−24.8%**) | 21,924 / 26,808 B (**−18.2%**) | 18 / 11 |

The ctx agent used the wrapper for all 18 commands with no protocol violations. It read PIRA help once, adding 3,229 output bytes already included above, and used exact mode for the complete authoritative design. More commands did not imply more context: its median command response was 260 B versus 1,483 B for the baseline, while status-only checks and a failed test retained bounded reports. However, this is one development trial rather than a causal estimate. The agents chose different implementations: the baseline produced 1,348 source/test lines and 14 self-tests, versus 1,086 lines and six self-tests for the ctx agent, although both passed every hidden case. That behavioral variance may explain part of the token and time difference. The valid ctx run was executed first, so run order did not give the tool a later prompt-cache advantage.

#### `pira_codenav`: repository inspection

Each navigation condition was one independent `gpt-5.6-sol` high-reasoning run against a pinned, read-only PyTorch or JAX checkout. Tool and no-PIRA conditions received the same source questions; baselines could use all ordinary read-only shell and Python facilities. Both JAX conditions received the same pinned Pyright server. All four runs had no tool-protocol violations.

| Repository | Answer correctness, tool / baseline | Evidence (precision / site recall), tool vs baseline | Total tokens | Noncached tokens | Wall time | Output bytes | Commands, tool / baseline |
|---|---:|---:|---:|---:|---:|---:|---:|
| PyTorch | 100% / 100% | (93.3% / 100%) vs (93.3% / 100%) | −21.1% | +51.9% | +6.5% | +41.7% | 9 / 13 |
| JAX | 100% / 98.4% | (64.5% / 76.9%) vs (69.4% / 100%) | −24.0% | −41.4% | −19.1% | −28.9% | 18 / 19 |

`pira_codenav` is a selective companion to `rg` and bounded reads rather than a blanket replacement. It reduced total tokens on both repositories. On JAX, one batched LSP query also improved answer correctness and reduced noncached tokens, wall time, and output, although the answer cited fewer reference sites. On PyTorch, fewer commands reduced repeated cached context, while preemptively enlarged search bounds still increased noncached tokens, output, and wall time. This is useful but not a Pareto improvement on every metric.

Token totals include repeated cached context; noncached tokens separate that effect. Output bytes measure tool-visible terminal output rather than model context directly. The context result is development protocol 1; navigation uses `pira_codenav` protocols 16 and 7. Every condition is a single trial, useful for diagnosing agent behavior but not a statistical or held-out performance claim.

</details>

### `pira_ctx`: lightweight command context

Long build logs, test output, and file listings can consume a model's working context. `pira_ctx` returns short output normally, but stores large output locally and gives the agent a compact summary. The full evidence remains available for focused follow-up.

<details>
<summary>Technical behavior, security, comparison, and validation</summary>

Automatic mode works as follows:

1. Ordinary short output is returned directly.
2. Long or diagnostic output is stored locally, while the model receives a short evidence-based summary and a capture ID.
3. The agent can later search, inspect a range, analyze, or replay the retained output.

For complete stdout-only JSON up to 512 KiB, the compact view exposes bounded scalar fields, small containers, and collection sizes before a few line excerpts. This keeps structured analysis results useful without replaying large arrays; the exact JSON remains stored.

Streams remain in memory through 64 KiB and spill only once when they grow larger or a live checkpoint needs an append-only path. Rebuildable event indexes avoid synchronous durability barriers; exact captures and authoritative event records remain durably published.

For jobs where only the outcome matters—such as builds, tests, or linting—`check` stores the log but returns a single PASS/FAIL line, exit code, and capture ID.

`exact` mode normally returns output unchanged. In non-interactive use, it may replace extremely repetitive or oversized output with a retained report rather than flood or silently truncate the context. It always announces that switch.

For a non-interactive program still running after about 30 seconds, `pira_ctx` publishes a read-only checkpoint. The agent can inspect a consistent snapshot without blocking the program. Running captures cannot be verified, deleted, or pruned until the program finishes.

Each recorded event keeps the purpose of a command without copying its output. Thread identifiers are combined with the workspace identity and hashed; raw identifiers are never stored. History defaults to the current thread, while workspace scope can combine anonymously labeled threads. Search is bounded and supports time windows, event windows, literal text, and explicit regular expressions. It does not silently perform fuzzy or semantic matching.

Version 1.0 stores checked, write-once event records and builds disposable per-thread search catalogs. If a catalog is missing or corrupt, it is rebuilt from the authoritative records. Older JSON ledgers are preserved but ignored until the user explicitly removes them.

Setup installs a verified native executable in the user's `PATH`. Normal use needs no Python, Rust toolchain, daemon, database, network, or model call. The optional `exec` analysis command uses Python 3; it can analyze up to 32 labeled captures together and accepts multiline analysis through stdin without temporary user files. Captures are private user-cache files with compressed, integrity-checked blocks. `pira_ctx` keeps the caller's permissions and does **not** sandbox commands. Run `pira_ctx --help` for usage. Source is under `tools/src/pira_ctx`; verified builds are under `tools/dist/pira_ctx`.

#### Security design

`pira_ctx` treats program output as untrusted data. It protects the capture and display path, but it is not a sandbox and cannot make the wrapped program safe:

- **Injection-aware display.** Agent-facing extracts are labeled as PROGRAM data, which PIRA rules treat as untrusted, and use trusted line and stream prefixes. Terminal escapes, Unicode line separators, bidirectional overrides, and invisible direction controls are sanitized so output cannot forge report structure or manipulate normal automatic display. A bounded heuristic scans the final displayed text for reserved role/wrapper markers and common **English** injection keywords, including English instructions split across displayed lines. Keyword detection is not multilingual; non-English text is detected only when it also contains a recognized marker or unsafe display control. When triggered, one warning appears before the evidence. Detection never suppresses or re-ranks evidence, and benign output pays no warning-token cost.
- **Explicit exactness.** Automatic mode retains short output matched by the advisory heuristic instead of replaying it directly; short `exec` output follows the same routing. `search` applies the same warning. Exact byte-replay paths in `exact`, `raw`, and `range` remain unsanitized because utility and faithful recovery take precedence; they remain untrusted data under PIRA's agent rules.
- **Bounded space, unbounded time.** Retention defaults to 512 MiB and 1,000,000 indexed lines, with a 2,000,000-line hard ceiling, while eager Python `exec` materialization defaults to 64 MiB. These ceilings are configurable within their safety bounds. Excess output is drained but not retained, and the command continues. `pira_ctx` imposes no runtime timeout or time-based termination, leaving cancellation to the agent or user.
- **Private, checked storage.** Captures use private user-cache files, independently compressed and SHA-256-checked blocks, validated offsets and lengths, and authenticated metadata/index tables. Common secret-bearing command arguments are redacted from metadata, and result IDs do not derive from raw arguments. Output may still contain secrets, and integrity hashes detect corruption rather than authenticate data against a same-user attacker.

Security checks are separate from ordinary functional tests and run as fixed, non-destructive fixtures in a deny-by-default sandbox with deliberately tiny configurable limits. Against 0.7.1 on 45 held-out benign real logs, 0.8.0 produced no false warnings, returned byte-identical responses, and showed no measurable median runtime regression in an alternating comparison. The live concurrency contract was also exercised with an inert delayed program in an isolated Linux Docker Sandbox. This is best-effort hardening, not a guarantee that every adversarial instruction will be detected; the primary boundary is the rule that PROGRAM output is data and cannot grant authority.

#### Relationship to Context Mode

`pira_ctx` was informed by [Context Mode](https://github.com/mksglu/context-mode), especially its ideas of keeping raw tool output out of context, attaching intent to execution, retrieving indexed evidence after compaction, and analyzing stored output with small programs. We thank its contributors for publishing and explaining these ideas.

| Dimension | `pira_ctx` | Context Mode |
|---|---|---|
| Integration | Native wrapper for explicit external commands | MCP server plus platform plugins and hooks |
| Runtime and storage | One Rust executable and self-contained checked capture files | Node/Bun integration with a SQLite FTS5 knowledge base |
| Reach | Commands deliberately routed through the wrapper | Broader shell, file, web, and MCP routing where integrations support it |
| Continuity | Bounded current-thread recap after compaction | Explicit session lifecycle and continuation support |
| Safety scope | Preserves caller permissions; does not sandbox children | Adds sandbox and permission-policy integration |

PIRA uses `pira_ctx` when a small single-binary wrapper and exact local fallback are preferable. Context Mode is the more comprehensive option when broader interception, hooks, sandboxing, or database-backed retrieval are needed.

#### Comprehensive held-out benchmark

The fixed benchmark caps each category at five cases and contains **45 sanitized responses across ten categories**. Its individual fixture contents were not seen during development of the output-selection design and were not used to tune selection, scoring, thresholds, injection heuristics, live checkpointing, or structured-JSON rendering; the fixed runner served as a regression and final measurement gate. The table reports `pira_ctx 1.4.0` on that corpus:

| Suite | Cases | Holdout source |
|---|---:|---|
| Public-repository core | 25 | New outputs generated after the freeze from ten fixed Rust repositories |
| Remote status workloads | 15 | Previously unseen Codex outputs streamed from a remote machine after the freeze |
| arXiv LaTeX supplement | 5 | Isolated builds of seeded recent arXiv papers, including natural and controlled failures |

The remote importer scanned raw logs in memory and persisted only fixed-point sanitized, privacy-audited fixtures; unsanitized server output was not written locally. Final selection is independent of PIRA output: SHA-256 order with a five-case cap, while build and test categories prefer three successes and two failures. No output routing, scoring, threshold, or security behavior changed after the final reported replay.

| Mode on the same 2,248,456 raw bytes | Returned context | Complete stored state | Median overhead | Immediate labeled evidence |
|---|---:|---:|---:|---:|
| `pira_ctx 1.4.0` automatic synopsis | 44,222 B (98.0% reduction) | 628,056 B (72.1% reduction) | +20.0 ms | 5/13 |
| Context Mode generic passthrough | 71,621 B (96.8% reduction) | 17,039,820 B (657.8% overhead) | +16.1 ms | 9/13 |
| `pira_ctx 1.4.0 check` | 3,064 B (99.9% reduction) | 628,191 B (72.1% reduction) | +20.3 ms | N/A—status only |
| Context Mode `ctx_index` receipt | 7,843 B (99.7% reduction) | 13,992,387 B (522.3% overhead) | N/A—no corresponding raw baseline | 0/13 |

All 45 PIRA cases preserved child status, entered full automatic-summary mode, reconstructed every sanitized output exactly, and passed integrity verification. Suggestions correctly abstained in 32/32 successful unlabeled cases; immediate evidence covered 5/8 failure markers and 0/5 changed basenames. Version 1.4.0 retains the frozen selection and quality counts. Context Mode generic passthrough classified all 45 recorded statuses correctly and immediately exposed 7/8 failure markers plus 2/5 changed basenames. These quality figures were not used for tuning.

<details>
<summary>Benchmark method, category results, Context Mode comparison, and limitations</summary>

##### Corpus and evaluation protocol

The prospective public core covers VCS patches, largest tracked Rust files, recursive declaration listings, 40-commit terminal logs, and GitHub pull-list responses. Exact and structural duplicates against earlier private corpora were excluded. Public changed basenames were preserved as sanitized metadata so suggestion labels remained observable. Five cases per category were selected by content SHA-256, producing 25 core cases.

The remote extension was fixed before inspecting output content. It reconstructed completed `exec_command` and `write_stdin` sessions from 2.73 GB of authorized Codex logs, streamed 683 category candidates through an in-memory sanitizer, retained 289 eligible unique responses, and selected cases by outcome, size bucket, session diversity, and content hash. The final five-case cap retained three successful and two failed builds, three successful and two failed tests, four setup/install responses, and one static-analysis response. The server contained no LaTeX response above the 2 KiB threshold.

LaTeX coverage therefore uses arXiv sources compiled inside an isolated Linux Docker Sandbox with TeX Live and shell escape disabled. Candidate papers came from a binary-seeded shuffle of the recent `cs.LG` API pool. Repeated transport interruptions caused the live recent-entry pool to drift, so the five already downloaded public identifiers were frozen before corpus persistence or PIRA evaluation. One paper compiled successfully; its fresh source also produced a controlled undefined-command failure. Three additional papers contributed natural compilation failures, yielding one pass and four failures. Raw paper sources were disposable and were not committed.

Each suite's output-quality labels were fixed during the original holdout evaluation and were not revised for 1.4.0. Deterministic byte/storage figures were identical across three 1.4.0 replays of the selected 45 fixtures through persistent automatic and `check` stores. Every call used an identical raw fixture-emitter baseline; overhead is `wrapped wall time - raw-operation wall time`. The table reports the median of the three per-replay case medians. Stored state includes captures, indexes, and event history but excludes installed binaries and runtimes.

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

##### Context Mode comparison on the final corpus

Context Mode 1.0.169 was installed inside an isolated Linux Docker Sandbox and rerun without errors on the exact final 45 sanitized fixtures. Generic passthrough used one persistent server, `ctx_execute_file`, the same category-level intent as PIRA, and JavaScript that printed each fixture while preserving its recorded exit status. Its direct Node emitter produced the same bytes and exit status as its raw baseline, so Docker startup and server initialization are excluded from overhead. It returned 71,621 bytes, classified all 45 statuses correctly, and immediately exposed 9/13 labeled outcomes: 7/8 failure markers and 2/5 changed basenames.

`ctx_index` used a separate persistent server and returned 7,843 bytes of indexing receipts. It exposed none of the 13 labels immediately, while exact content remained available through later search. Indexing has no equivalent raw operation, so no synthetic latency overhead is reported. Both Context Mode storage figures include its SQLite FTS5 retrieval state after shutdown; installed packages are excluded.

Generic passthrough is the closest automatic wrapper-level comparison, not Context Mode's recommended workflow. Context Mode normally asks the model to run task-specific analysis code and return only the derived answer. Its [published benchmark](https://github.com/mksglu/context-mode/blob/main/BENCHMARK.md) reports 98% reduction for task-specific execution, 82% for exact index-plus-search retrieval, and 96% overall. Returned-context measurements here count UTF-8 bytes rather than tokenizer-specific tokens, and immediate visibility does not measure evidence recoverable by later search.

##### Limitations

This remains a private implementation benchmark on one arm64 macOS evaluation host, not a universal performance claim. The remote suite is genuinely unseen and post-freeze imported, but its logs predate the freeze and are therefore not prospective outputs. Setup/install and static-analysis coverage remains below the five-case cap because no more eligible unique remote responses were available. Failure markers measure visibility of broad outcome evidence rather than complete diagnostic usefulness. arXiv selection required baseline build availability and includes one intentionally mutated source. Privacy sanitation changes path separators in LaTeX logs. Binary, non-UTF-8, and interactive-terminal behavior are covered by functional tests rather than this corpus. Web-search returns remain excluded because Codex built-in web output is not directly captured by the local command wrapper.

</details>

</details>

### `pira_decision`: structured decision memory

Projects often lose the reason behind a choice. `pira_decision` keeps the context, serious alternatives, selected option, decision-maker, and time in one searchable record.

<details>
<summary>Technical behavior, storage, and concurrency</summary>

Each record contains an ID, UTC timestamp, short context, ordered alternatives, the selected option, and whether the decision came from a human, an agent, or both. Saved records are not edited in place.

The tool can add a decision, show one record, search individual fields with regular expressions, or explicitly forget one exact record. Search can distinguish every considered choice from the option that was selected, and returns newer matches first.

Records live in private per-user application data and are separated by workspace. Multiple agents can write safely at the same time. Readers see only complete records, although a decision saved during a search may not appear until the next search. Invalid unrelated records are reported and skipped. No SQL database, daemon, network service, or repository-local metadata is required.

Public source is under `tools/src/pira_decision`. Build it with `cargo build --manifest-path tools/Cargo.toml -p pira_decision --release`, then run `pira_decision --help` or command-specific help for exact usage.

</details>

### `pira_codenav`: lightweight code navigation

Reading an entire repository is slow and context-heavy. `pira_codenav` helps an agent narrow repository shape, declarations, implementation text, relationships, and exact source. It is read-only: it never edits or executes repository code, and it is not a mandatory replacement for a bounded read when the exact file and range are already known.

<details>
<summary>Technical behavior, language support, security, and validation</summary>

Choose the cheapest operation that can return enough evidence:

1. Use ordinary `rg` or a bounded read for exact body text or line ranges already narrowed to known paths.
2. `find` batches declaration names, tries exact names before substring fallback, ranks public/close matches first, and includes small unique source automatically.
3. `outline` gives a known file's declarations without bodies; structural `show` selects a named item, while parser-free `show` can validate selectors or batch exact spans.
4. `map` gives a bounded repository or subsystem shape when relevant files are unknown.
5. `imports`, `dependents`, and `deps` expose conservative file relationships without a build system.
6. Semantic commands such as `definition`, `references`, `callers`, and `hover` use a caller-installed language server; `query` mixes up to 32 operations while sharing servers and open documents.
7. `search` is the bounded language-filtered alternative when merged context and clean enclosing-item annotations add value beyond plain text matching.

It supports 23 languages: Python, Rust, Java, C, C++, CUDA, Bash, Go, JavaScript, TypeScript/TSX, C#, PowerShell, PHP, Kotlin, Lua, HCL/Terraform, R, Ruby, Swift, Scala, Dart, Elixir, and Julia. All native parsers are built into one executable. Explicit native mode needs no language runtime, daemon, database, network, project initialization, package manager, or runtime download. Run `pira_codenav --help` for usage.

Output stays compact by omitting routine success details. Default `search` returns at most 48 ranked matches and 24 KiB with one context line, multi-target `show` uses 32 KiB, and `map` returns at most 200 balanced files. Explicit limits can be raised when needed. Failures, incomplete results, omitted items, truncation, ambiguity, unsupported files, and language-server use remain explicit.

#### Native and LSP backends

File relationships, language detection, and implementation-text search do not need a language server. `outline`, structural `show`, `map`, and `find` instead require and prefer a suitable PATH-discovered or explicit LSP by default. This matches active code-inspection environments and keeps syntax-broken work on the more authoritative backend. Because standard `documentSymbol` responses often omit imports and module bindings, clean built-in parsing cheaply supplements only missing declaration names while LSP symbols remain primary; dirty files use LSP alone. `--no-lsp` is the single explicit opt-out: it uses only the built-in Tree-sitter parsers and rejects syntax-dirty files rather than presenting best-effort recovery as clean.

Semantic commands use one-based `FILE:LINE:COLUMN` positions and never fall back to textual guesses. Up to 32 same-operation targets share one invocation; `query` accepts mixed `OPERATION=FILE:LINE:COLUMN` requests. Both reuse matching language servers and opened documents. Optional JSON files can provide server initialization and workspace settings.

PIRA checks only fixed conventional dedicated server names such as `pyright-langserver`, `rust-analyzer`, or `clangd` on `PATH`; it never derives an executable from repository text. Explicit `--lsp` configuration overrides discovery. If no matching server exists, structural navigation stops with a concise warning and points to `--no-lsp`. Servers are reused within one invocation and then shut down. PIRA keeps no daemon or persistent code index, and processes repositories in bounded batches.

Batch commands keep useful successful results even when some files fail. They clearly report incomplete processing, errors, and omitted output. If everything fails, the command returns the underlying failure.

#### Relationship to ast-outline and Grove

ast-outline and Grove are useful functional baselines for compact structure and broad-to-narrow retrieval. PIRA keeps those ideas while avoiding runtime/project initialization and persistent state, and adds conservative file relationships plus a standard optional-LSP path. The comparison includes only overlapping clean tasks with task-specific correctness assertions. Unsupported or empty operations are omitted rather than counted as fast results; output schemas and exact retrieval semantics differ.

#### Read-only and security boundary

- Repository code is parsed but never executed or edited. Ignore rules are honored, symlinked directories are not followed, and dependency targets outside the selected root are blocked.
- Exact source and LSP hover are framed as untrusted data. Unsafe terminal controls are escaped in source, hover, paths, symbols, signatures, dependency/call metadata, and errors.
- PIRA rejects `workspace/applyEdit`. An explicit or PATH-discovered server remains an external executable and may maintain its own caches; trust PATH as executable configuration or select built-in parsing with `--no-lsp`.
- Source, syntax depth, regex compilation, LSP messages/headers, configuration files, symbols, locations, call relations/ranges, hover, stderr, and reported errors are bounded. PIRA imposes no command timeout; the caller controls cancellation.

#### Validation

| Property | Result |
|---|---:|
| Supported languages | 23 |
| Public correctness files | 74 |
| Explicit-native clean / correctly rejected dirty files | 67 / 7 |
| Native structural targets | 1,047 |
| Location / freshness-selector round trips | 1,047/1,047 each |
| Curated essential-target recall | 72/72 |
| Functional / inert security / Rust tests | 89 / 17 / 14 |
| Reproducible benchmark tasks | 40 |

The retained Linux arm64 sandbox validates clangd 21.1.8 for definitions and incoming/outgoing call hierarchy, and basedpyright 1.39.9 for definition, implementation, type-definition, references, and hover. Deterministic fake-server tests additionally cover multi-target process reuse, initialization/settings forwarding, call-site normalization, independent capabilities, UTF-16 positions, rejected edits, malformed/oversized/hostile protocol data, lazy startup, and cached startup/parse failures.

#### Performance

Each latency is a complete subprocess call through collected output. Native macOS arm64 measurements use the 0.6.0 release with 10 warmups and 100 calls. Same-sandbox Linux arm64 measurements use 5 warmups and 40 calls inside an already-running 2-CPU/4-GiB Docker Sandbox. Those retained columns measure overlapping 0.4.1 commands and do not include `query`; cross-tool comparisons use only same-sandbox columns. Host and sandbox timings describe different environments and are not compared directly.

| Clean operation | PIRA native | PIRA sandbox | ast-outline 1.8.2 sandbox | Grove 0.3.1 sandbox |
|---|---:|---:|---:|---:|
| Python outline | 5.075 ms | 2.238 ms | 51.226 ms | 40.466 ms |
| Rust outline | 6.719 ms | 3.348 ms | 52.752 ms | 42.178 ms |
| Python exact item | 5.565 ms | 2.065 ms | 50.656 ms | 40.508 ms |
| Python repository map | 4.153 ms | 0.927 ms | 49.323 ms | 35.797 ms |
| Rust repository map | 4.214 ms | 0.860 ms | 48.708 ms | 33.463 ms |
| Java outline | 3.632 ms | 0.951 ms | 47.728 ms | 29.241 ms |
| C outline | 2.792 ms | 0.351 ms | unsupported | 71.696 ms |
| C++ outline | 2.775 ms | 0.399 ms | 46.996 ms | 118.023 ms |

On these tasks, the fastest available baseline took 12.6–204× as long as PIRA in the same sandbox. PIRA used about 4.1–5.6 MiB peak RSS, versus about 15.6–49.7 MiB for the available baselines. The largest ratios use tiny synthetic C/C++ fixtures and principally measure complete-call overhead.

##### LSP cost

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

On the native 0.6.0 release, one `query` combining definition, references, and hover took 33.268 ms median. The same three complete calls totaled 96.863 ms by their individual medians, so shared startup and document state reduced latency by 65.7% (2.91×).

##### Subcommand context and latency

Context reduction compares returned UTF-8 bytes with complete source bytes otherwise needed by the fixed task. It is not a tokenizer-specific token count. Semantic rows use a deterministic minimal protocol server to isolate wrapper behavior rather than production-server initialization.

| Subcommand | Returned bytes | Context reduction | Native median | Sandbox median | Sandbox peak RSS |
|---|---:|---:|---:|---:|---:|
| `outline` | 1,652 | 92.4% | 5.075 ms | 2.238 ms | 4.1 MiB |
| `show` | 3,334 | 84.6% | 5.565 ms | 2.065 ms | 4.1 MiB |
| `map` | 432 | 69.9% | 4.153 ms | 0.927 ms | 4.6 MiB |
| `find` | 650 | 54.6% | 4.430 ms | 0.976 ms | 4.6 MiB |
| `search` | 2,313 | 89.3% | 6.389 ms | not measured | not measured |
| `imports` | 389 | 56.6% | 3.034 ms | 0.437 ms | 4.1 MiB |
| `dependents` | 228 | 84.1% | 4.262 ms | 0.838 ms | 4.6 MiB |
| `deps` | 342 | 76.1% | 4.240 ms | 0.818 ms | 4.6 MiB |
| `definition` | 147 | 99.3% | 32.674 ms | 16.303 ms | 12.7 MiB |
| `implementation` | 151 | 99.3% | 32.504 ms | 17.214 ms | 12.7 MiB |
| `type-definition` | 152 | 99.3% | 31.929 ms | 16.539 ms | 12.7 MiB |
| `references` | 1,435 | 93.4% | 32.134 ms | 16.461 ms | 12.7 MiB |
| `callers` | 237 | 98.9% | 31.570 ms | 15.728 ms | 12.7 MiB |
| `callees` | 237 | 98.9% | 31.659 ms | 16.063 ms | 12.7 MiB |
| `hover` | 200 | 99.1% | 32.055 ms | 15.920 ms | 12.7 MiB |
| `query` | 2,075 | 90.4% | 33.268 ms | not measured | not measured |
| `languages` | 166 | not applicable | 2.710 ms | 0.269 ms | 2.6 MiB |

`outline`, `show`, `search`, and semantic/query rows use the complete 21,680-byte pinned Click file. `find`, `map`, `dependents`, and `deps` use all 1,433 supported source bytes in the deterministic Python repository; `imports` uses its 896-byte file.
The `find` row includes its small unique declaration source by default, trading some single-call bytes for one fewer retrieval round trip; `--locations-only` keeps only ranked locations.

##### All-language outline regression

| Language | Fixture | Source | Outline | Reduction | Native | Sandbox |
|---|---|---:|---:|---:|---:|---:|
| Python | Click | 21,680 B | 1,652 B | 92.4% | 5.075 ms | 2.238 ms |
| Rust | ripgrep | 32,269 B | 2,884 B | 91.1% | 6.719 ms | 3.348 ms |
| Java | JUnit | 12,572 B | 1,371 B | 89.1% | 3.632 ms | 0.951 ms |
| C | synthetic | 210 B | 124 B | 41.0% | 2.792 ms | 0.351 ms |
| C++ | synthetic | 202 B | 272 B | −34.7% | 2.775 ms | 0.399 ms |
| CUDA | synthetic | 419 B | 198 B | 52.7% | 2.868 ms | 0.430 ms |
| Bash | bats-core | 16,510 B | 291 B | 98.2% | 4.530 ms | 2.613 ms |
| Go | synthetic | 119 B | 99 B | 16.8% | 2.812 ms | 0.288 ms |
| JavaScript | synthetic | 181 B | 211 B | −16.6% | 2.837 ms | 0.311 ms |
| TypeScript | synthetic | 386 B | 214 B | 44.6% | 2.934 ms | 0.353 ms |
| C# | synthetic | 235 B | 239 B | −1.7% | 2.936 ms | 0.382 ms |
| PowerShell | PowerShell | 17,197 B | 1,519 B | 91.2% | 4.857 ms | 10.194 ms |
| PHP | Laravel | 55,672 B | 7,444 B | 86.6% | 8.360 ms | 4.612 ms |
| Kotlin | kotlinx.coroutines | 17,043 B | 795 B | 95.3% | 3.498 ms | 0.986 ms |
| Lua | Neovim | 53,653 B | 2,071 B | 96.1% | 7.897 ms | 4.361 ms |
| HCL | Terraform | 2,248 B | 2,006 B | 10.8% | 3.303 ms | 0.667 ms |
| R | dplyr | 15,796 B | 712 B | 95.5% | 4.606 ms | 1.791 ms |
| Ruby | Rails | 14,867 B | 142 B | 99.0% | 3.535 ms | 1.154 ms |
| Swift | ArgumentParser | 10,088 B | 1,233 B | 87.8% | 4.299 ms | 4.963 ms |
| Scala | cats-effect | 82,458 B | 13,913 B | 83.1% | 27.233 ms | 22.573 ms |
| Dart | http | 3,856 B | 550 B | 85.7% | 3.833 ms | 1.087 ms |
| Elixir | Elixir | 42,726 B | 3,237 B | 92.4% | 8.179 ms | 9.124 ms |
| Julia | HTTP.jl | 4,675 B | 131 B | 97.2% | 3.688 ms | 1.072 ms |

Negative reduction means fixed structural metadata exceeds a tiny source fixture; it does not indicate lost source. This table is a parser-path regression check, not a repository-scale compression claim.

A minimal `pira_codenav --version` call measured 2.781 ms median / 3.274 ms p95 on native macOS. The optimized macOS arm64 binary is 51,307,488 bytes, or 5,779,854 bytes with deterministic gzip level 9.

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

## Why this design

PIRA is intentionally small and inspectable:

- **Inspectable.** Behavior is organized in readable policy and module files that are easy to review and customize.
- **Lightweight.** It avoids a heavy framework and keeps instruction overhead low.
- **Research-oriented.** Default workflows emphasize reasoning, writing, coding, evidence gathering, and careful iteration.
- **Lean by default.** Inspired by [Ponytail](https://github.com/DietrichGebert/ponytail) and general lessons from *Clean Code* and *Clean Architecture*, its coding style prefers simple, safe implementations and clear boundaries over speculative abstractions.
- **Tool-friendly.** The small, explicit design integrates naturally with official tools such as Codex.

## Safety model

<details>
<summary>Permission boundaries and operating rules</summary>

PIRA can run with full system permissions, but full-permission mode is not a sandbox. Its rules require the agent to:

- review the action, scope, destructive risk, privacy impact, and rollback path before a state-changing command;
- prefer narrow, reversible actions;
- avoid destructive commands without explicit permission;
- keep temporary artifacts in the platform temp directory unless the user wants them preserved.

Codex subagents load the same policy as the main agent. This behavior has not been equally tested on other agent platforms.

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

Most PIRA instruction files use **Meaning-Preserving Telegraphic Compression (MPTC)**: filler and repetition are removed, but each rule keeps who acts, what is required, when it applies, its scope, and its exceptions. Safety and permission rules stay fully grammatical. The initial tracked-file pass reduced instruction size by **20.0%** and whitespace-delimited word count by **26.0%**; actual token savings depend on the tokenizer.

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
