# PIRA

PIRA (pronounced "Pyra") is a research-oriented personal agent for reasoning, writing, coding, learning, and practical problem-solving.

It adds a small, inspectable set of instructions and tools to coding agents. The goal is simple: make the agent more careful, useful, and consistent without hiding how it works.

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

PIRA has been tested extensively with **Codex on GPT-5.4, GPT-5.5, and 5.6-sol, each using high reasoning effort**. Claude Code support is experimental and has not received the same level of testing.

## Quick start

PIRA installs to `~/agent` by default. You can use the one-line command for the easiest setup, or the inspect-first path if you want to review every change before it happens.

Setup is safe to rerun: it preserves an existing `USER.md`, backs up user-level Codex files before changing them, and can preview or verify its work. Git is required. The setup helper checks for Python and can offer platform-specific installation help.

### Recommended one-line install or update

The recommended command installs or updates PIRA, then connects it to Codex. It:
- uses the existing `~/agent` git checkout when present, otherwise clones PIRA into `~/agent`;
- enables **soft-safe** mode;
- keeps audio notifications **off**;
- configures Codex to load PIRA's canonical `AGENTS.md` once;
- installs or refreshes bundled PIRA tools in the user's `PATH`;
- moves old PIRA-managed legacy files into backup;
- creates a private `USER.md` placeholder only if `USER.md` is missing.

macOS/Linux:

```bash
if [ -d ~/agent/.git ]; then cd ~/agent && git pull --ff-only; else git clone https://github.com/AlgebraLoveme/PIRA.git ~/agent && cd ~/agent; fi && assets/scripts/setup_pira.sh --yes --execution-mode soft-safe --audio no --user-mode placeholder --legacy remove
```

Windows PowerShell:

```powershell
if (Test-Path "$HOME/agent/.git") { Set-Location "$HOME/agent"; git pull --ff-only; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE } } else { git clone https://github.com/AlgebraLoveme/PIRA.git "$HOME/agent"; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; Set-Location "$HOME/agent" }; powershell.exe -ExecutionPolicy Bypass -File assets/scripts/setup_pira.ps1 --yes --execution-mode soft-safe --audio no --user-mode placeholder --legacy remove
```

If you are updating PIRA and intentionally do not use `USER.md`, choose `--user-mode keep`:

macOS/Linux:

```bash
cd ~/agent && git pull --ff-only && assets/scripts/setup_pira.sh --yes --execution-mode soft-safe --audio no --user-mode keep --legacy remove
```

Windows PowerShell:

```powershell
Set-Location "$HOME/agent"; git pull --ff-only; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; powershell.exe -ExecutionPolicy Bypass -File assets/scripts/setup_pira.ps1 --yes --execution-mode soft-safe --audio no --user-mode keep --legacy remove
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

### Claude Code (experimental)

Claude Code reads `CLAUDE.md`, not `AGENTS.md`. PIRA follows Claude Code's documented compatibility pattern: the user-level `~/.claude/CLAUDE.md` contains one managed import of the canonical `~/agent/AGENTS.md`, followed by two short Claude-specific reminders for module loading and Bash routing. It does not copy the policy into a second tree or install duplicate skills.

Run these commands after the checkout is available at `~/agent`, which is also where the imported modules live. If another directory already occupies `~/agent`, setup intentionally stops rather than replacing it; resolve that conflict explicitly before continuing. Then preview, install, and verify with:

macOS/Linux:

```bash
assets/scripts/setup_pira.sh --claude-code --dry-run --skip-tools
assets/scripts/setup_pira.sh --claude-code --yes --user-mode placeholder --legacy remove
assets/scripts/setup_pira.sh --claude-code --verify
```

Windows PowerShell:

```powershell
powershell.exe -ExecutionPolicy Bypass -File assets/scripts/setup_pira.ps1 --claude-code --dry-run --skip-tools
powershell.exe -ExecutionPolicy Bypass -File assets/scripts/setup_pira.ps1 --claude-code --yes --user-mode placeholder --legacy remove
powershell.exe -ExecutionPolicy Bypass -File assets/scripts/setup_pira.ps1 --claude-code --verify
```

The Claude Code mode preserves content outside the PIRA-managed block, backs up `CLAUDE.md` before changing it, and uses the same three PIRA tools as Codex. The small bridge repeats only the two entry rules that Claude Code must apply before the imported policy can govern tool use; it does not duplicate PIRA's modules. Setup does not change Claude Code permission settings or install Codex audio hooks.

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
| `--claude-code` | Configures Claude Code instead of Codex by managing one import in `~/.claude/CLAUDE.md`. |
| `--claude-md PATH` | Overrides the Claude Code user instruction file used with `--claude-code`. |
| `--yes` | Accepts setup confirmations. It does **not** enable audio unless `--audio yes` is also set. |
| `--audio yes\|no\|ask` | Controls optional Codex audio notifications. Use `--audio no` for a quiet install. |
| `--legacy remove\|keep\|ask` | Controls paths listed in `assets/LEGACY_LIST.md`; `remove` moves active legacy files into `.backup/setup_pira_legacy/`. |
| `--agent-dir PATH` | Installs against a path other than `~/agent`. |
| `--skip-tools` | Skips installation or refresh of released native PIRA tools. |
| `--tools-install-dir PATH` | Overrides the per-user tools directory (`~/.local/bin` on macOS/Linux or `%LOCALAPPDATA%\PIRA\bin` on Windows). |
| `--tools-version TOOL=VERSION` | Pins one tool version, for example `ctx=1.7.0`; repeat for multiple tools. Unspecified tools use the latest release. |
| `--verify` | Checks the current setup without writing. |
| `--dry-run` | Prints planned changes without applying them. |

### Install or refresh only the PIRA tools

If PIRA itself is already configured, these commands update only its native tools. A normal run downloads the latest GitHub Release for this platform, installs missing tools, replaces outdated copies, and leaves verified matching copies unchanged.

On macOS or Linux:

```bash
cd ~/agent
python3 assets/scripts/setup_pira_tools.py          # install or refresh
python3 assets/scripts/setup_pira_tools.py --force  # reinstall the selected release
python3 assets/scripts/setup_pira_tools.py --verify # verify without writing
python3 assets/scripts/setup_pira_tools.py --version ctx=1.7.0 --version nav=0.12.0
```

On Windows PowerShell:

```powershell
cd $HOME\agent
py -3 assets/scripts/setup_pira_tools.py          # install or refresh
py -3 assets/scripts/setup_pira_tools.py --force  # reinstall the selected release
py -3 assets/scripts/setup_pira_tools.py --verify # verify without writing
py -3 assets/scripts/setup_pira_tools.py --version ctx=1.7.0 --version nav=0.12.0
```

The updater obtains binaries from `AlgebraLoveme/PIRA` GitHub Releases and verifies their recorded size, SHA-256 checksum, and reported tool version before installation. `--version ctx=VERSION`, `dec=VERSION`, or `nav=VERSION` selects a concrete cloud-built version and may be repeated; unspecified tools use the latest release. Exact-version history begins with this release system, so versions never published by it are reported as unavailable. `--tool NAME` limits installation to one tool and may be repeated. `--force` reinstalls an already matching copy; `--install-dir PATH` changes the destination; `--no-path` leaves PATH management to you. Setup needs network access; normal tool use does not. Restart the shell or agent process if setup says the new PATH is not active yet.

### Maintainer release procedure

PIRA has one source/install branch: `master`. Change tool source there and bump the affected Cargo package version, run local source tests, then push `master`. In GitHub Actions, manually run **Build PIRA tool bundles** from `master`. The owner-gated workflow runs the workspace tests natively on Windows, builds all five supported platforms twice, rejects non-reproducible output, and publishes a new GitHub Release containing versioned assets for all three tools. No local cross-platform build, generated-binary commit, or release branch is part of the procedure. After the workflow succeeds, a fresh clone of `master` plus the setup script installs the latest release automatically.

</details>

## What setup changes

<details>
<summary>Files, settings, tools, and verification performed by setup</summary>

In plain language, setup connects PIRA to the selected coding agent, installs its tools, and checks the result. More precisely, it:

1. Detects the repository directory and ensures it is available as `~/agent`, unless another `--agent-dir` is given.
2. Initializes a private `USER.md` placeholder when needed.
3. Moves legacy files listed in `assets/LEGACY_LIST.md` into `.backup/setup_pira_legacy/` when approved.
4. By default, updates or creates Codex `config.toml` so the selected agent directory's `AGENTS.md` is loaded, with `project_doc_max_bytes = 65536`. With `--claude-code`, it instead adds one managed import to the selected `CLAUDE.md` while preserving content outside that block.
5. In Codex mode, creates a local repository guard that prevents the same `AGENTS.md` from being rediscovered while working inside the PIRA checkout, and removes an older `~/.codex/AGENTS.md` symlink only when it duplicates PIRA.
6. Selects and verifies the bundled native tools for the current platform, then installs or refreshes them in a per-user PATH directory. Existing stale copies are atomically replaced; matching copies are left unchanged.
7. In Codex mode, optionally delegates audio setup to the platform-specific audio helper.
8. Verifies the setup, including the PIRA verification token and installed native tools.

If setup cannot safely handle an existing conflicting file or setting, it stops or skips that action with a warning instead of silently overwriting it.

</details>

## Memory System

PIRA separates memory by how long it should remain useful. This prevents raw command logs from crowding out important project knowledge.

| Layer | What it remembers | Where it lives | Who reads it |
|---|---|---|---|
| Activity | Commands, actions, and detailed evidence | `pira_ctx` | Agents search it when needed |
| Decisions | Important choices, alternatives, and reasons | `pira_dec` | Agents search it when needed |
| Project knowledge | Durable state, validated results, lessons, and limitations | `AGENT_WORKBOOK.md` | Agents and humans read it directly |

Detailed activity remains searchable for as long as retention allows. After a conversation is compacted, `pira_ctx recap` can restore the recent activity needed to continue the work; it is part of the activity layer, not another memory layer.

PIRA creates `AGENT_WORKBOOK.md` lazily at the workspace root only when the first durable project entry is warranted. It starts with only the needed headings and uses Git's local exclude file without changing the project's shared `.gitignore`.

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

Information moves upward only when its lasting value increases. Activity stays in `pira_ctx`; concluded choices go to `pira_dec`; durable consequences become self-contained workbook entries. The workbook never requires a reader to look up a lower-level record.

## PIRA Internal Tools

PIRA includes three small tools that help agents work with less noise and better continuity. Most users never need to run them manually; built-in command help provides exact syntax.

| Tool | What problem it solves |
|---|---|
| `pira_ctx` | Keeps large command output available without flooding the active conversation. |
| `pira_dec` | Records important choices in a consistent, searchable form. |
| `pira_nav` | Provides portable lexical, structural, dependency, and optional IDE-semantic repository navigation. |

### Agent-level evaluation

Isolated agentic benchmarks tested whether PIRA's internal tools help fresh agents complete real tasks, not merely whether individual binaries are fast in microbenchmarks.

<details>
<summary>Method, results, and interpretation</summary>

#### Joint tools: real-world Codex maintenance

Two fresh `gpt-5.6-sol` high-reasoning agents received the same pinned `openai/codex` source, issue-derived prompt, sanitized synthetic ultra-long rollout, upstream instructions, development toolchain, warmed caches, and one task turn. The task was an authentic, then-unsolved performance bug: capped session resume had to avoid formatting every historical terminal cell while preserving the complete ordered transcript, uncapped behavior, persisted data, and public defaults. The baseline received PIRA's common identity, safety, research, and coding policy; the tool condition received the same byte prefix plus the internal-tool rules and binaries. Neither condition received a solution or task-time web access.

A private behavior-based evaluator was added only after each agent finished. It checked the build and focused upstream tests, tail-first capped replay, unchanged uncapped replay, complete transcript retention, unchanged defaults and rollout data, formatting, and the agent's regression tests.

| Condition | Correctness | Base-rate cost estimate | Wall time | Command-output bytes | Commands | Frozen rule bytes |
|---|---:|---:|---:|---:|---:|---:|
| Baseline | 7/7 | \$7.504 | 1,908.6 s | 907,239 B | 72 | 14,328 B |
| PIRA tools | 7/7 | \$4.808 (**−35.9%**) | 1,824.2 s (**−4.4%**) | 234,285 B (**−74.2%**) | 100 | 22,836 B |

The estimate applies a fixed table matching standard GPT-5.6 Sol rates at run time—\$5 per million uncached input tokens, \$0.50 per million cached input tokens, and \$30 per million output tokens—to aggregate usage telemetry. It is not an observed invoice and does not model request-level long-context multipliers, cache-write pricing, tool charges, fast mode, included plan usage, or account-specific terms. PIRA issued more commands, including 34 `pira_ctx` and 66 `pira_nav` calls, but exposed substantially less command text and reduced the base-rate estimate. Its 8,508 additional instruction bytes are reported rather than subtracted through an unsupported token estimate. This is one controlled task and two agent trajectories, not a general causal estimate; it demonstrates a successful difficult case rather than guaranteed savings on every task.

#### `pira_ctx`: natural implementation task

Two fresh `gpt-5.6-sol` high-reasoning agents received byte-identical prompts and clean Git repositories containing only the current `pira_dec` design. Both were asked to implement and test the complete tool in Python. No command output, capture, prescribed workflow, existing implementation, or answer was supplied. The tool condition had only `pira_ctx 1.4.0` and its ctx-specific rules; the baseline had no PIRA rules or binaries, and neither condition had a navigation or decision binary. Both could write their isolated workspace and use ordinary Python and Git.

Each generated implementation was then run inside its sandbox against the same 20 private black-box cases covering validation, JSON views, search, workspace scope, the checked record envelope, corruption, symlink rejection, safe deletion, concurrency, bounds, and temporary-file isolation.

| Correctness, tool / baseline | Total tokens, tool / baseline | Noncached tokens | Wall time | Command-output bytes | Commands, tool / baseline |
|---:|---:|---:|---:|---:|---:|
| 20/20 / 20/20 | 670,031 / 819,876 (**−18.3%**) | 70,735 / 108,964 (**−35.1%**) | 690.2 / 918.2 s (**−24.8%**) | 21,924 / 26,808 B (**−18.2%**) | 18 / 11 |

The ctx agent used the wrapper for all 18 commands with no protocol violations. It read PIRA help once, adding 3,229 output bytes already included above, and used exact mode for the complete authoritative design. More commands did not imply more context: its median command response was 260 B versus 1,483 B for the baseline, while status-only checks and a failed test retained bounded reports. However, this is one development trial rather than a causal estimate. The agents chose different implementations: the baseline produced 1,348 source/test lines and 14 self-tests, versus 1,086 lines and six self-tests for the ctx agent, although both passed every hidden case. That behavioral variance may explain part of the token and time difference. The valid ctx run was executed first, so run order did not give the tool a later prompt-cache advantage.

</details>

### `pira_ctx`: lightweight command context

Long build logs, test output, and file listings can consume a model's working context. `pira_ctx` returns short output normally, but stores large output locally and gives the agent a compact summary. The full evidence remains available for focused follow-up.

<details>
<summary>Technical behavior, security, comparison, and validation</summary>

Automatic mode works as follows:

1. Ordinary output up to 4 KiB, or non-repetitive output up to 64 lines and 16 KiB, is returned directly.
2. Larger, repetitive, binary/non-UTF-8, risky, truncated, or live output is stored locally, while the model receives a short evidence-based summary and a capture ID.
3. The agent can later search, inspect a range, analyze, or replay the retained output.

When important wording is known beforehand, `--interest REGEX` puts matching indexed display lines
in a strict highest-priority tier for automatic or capture-mode synopses. The existing line weights
continue to rank matches among matches and nonmatches among nonmatches. Therefore, if a selected
synopsis includes a nonmatching line, no omitted indexed line matches the regex. Reported retention
or line-index truncation limits this guarantee because unretained or unindexed output cannot be
examined. Regexes use Rust syntax and are case-sensitive unless an inline flag such as `(?i)` is used;
an invalid regex is rejected before the wrapped program starts.

For complete stdout-only JSON up to 512 KiB, the compact view exposes bounded scalar fields, small containers, and collection sizes before a few line excerpts. This keeps structured analysis results useful without replaying large arrays; the exact JSON remains stored.

Streams remain in memory through 64 KiB and spill only once when they grow larger or a live checkpoint needs an append-only path. Rebuildable event indexes avoid synchronous durability barriers; exact captures and authoritative event records remain durably published.

For jobs where only the outcome matters—such as builds, tests, or linting—`check` stores the log but returns a single PASS/FAIL line, exit code, and capture ID.

`exact` mode normally returns output unchanged. In non-interactive use, it may replace extremely repetitive or oversized output with a retained report rather than flood or silently truncate the context. It always announces that switch.

For a non-interactive program still running after about 30 seconds, `pira_ctx` publishes a read-only checkpoint. The agent can inspect a consistent snapshot without blocking the program. Running captures cannot be verified, deleted, or pruned until the program finishes.

Each recorded event keeps the purpose of a command without copying its output. Thread identifiers are combined with the workspace identity and hashed; raw identifiers are never stored. History defaults to the current thread, while workspace scope can combine anonymously labeled threads. Search is bounded and supports time windows, event windows, literal text, and explicit regular expressions. It does not silently perform fuzzy or semantic matching.

Version 1.0 stores checked, write-once event records and builds disposable per-thread search catalogs. If a catalog is missing or corrupt, it is rebuilt from the authoritative records. Older JSON ledgers are preserved but ignored until the user explicitly removes them.

Setup downloads a checksum-verified native executable from GitHub Releases into the user's `PATH`. Normal use needs no Python, Rust toolchain, daemon, database, network, or model call. The optional `exec` analysis command uses Python 3; it can analyze up to 32 labeled captures together and accepts multiline analysis through stdin without temporary user files. Captures are private user-cache files with compressed, integrity-checked blocks. `pira_ctx` keeps the caller's permissions and does **not** sandbox commands. Run `pira_ctx --help` for usage. Source is under `tools/src/pira_ctx`.

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

The remote importer scanned raw logs in memory and persisted only fixed-point sanitized, privacy-audited fixtures; unsanitized server output was not written locally. Final selection is independent of PIRA output: SHA-256 order with a five-case cap, while build and test categories prefer three successes and two failures. The current routing policy was fixed from live tool-use findings before this post-change replay, and no behavior changed afterward.

| Mode on the same 2,248,456 raw bytes | Returned context | Retained state | Median overhead | Immediate annotated evidence |
|---|---:|---:|---:|---:|
| `pira_ctx 1.4.0` automatic routing | 67,672 B (97.0% reduction) | 590,920 B (73.7% reduction) | +20.3 ms | 8/13 |
| Context Mode generic passthrough | 71,621 B (96.8% reduction) | 17,039,820 B (657.8% overhead) | +16.1 ms | 9/13 |
| `pira_ctx 1.4.0 check` | 3,064 B (99.9% reduction) | 628,191 B (72.1% reduction) | +20.3 ms | N/A—status only |
| Context Mode `ctx_index` receipt | 7,843 B (99.7% reduction) | 13,992,387 B (522.3% overhead) | N/A—no corresponding raw baseline | 0/13 |

All 45 PIRA cases preserved child status and exact immediate content. Thirty-seven long or repetitive responses entered summary mode, were retained exactly, reconstructed, and passed integrity verification; eight bounded non-repetitive responses were replayed exactly and intentionally not retained. Across the 13 frozen marker/basename annotations, immediate output exposed 8, versus 9 for Context Mode generic passthrough. The one-point context-reduction trade-off buys direct readability for dense outputs that previously required a second retrieval. These quality figures were not used for tuning.

<details>
<summary>Benchmark method, category results, Context Mode comparison, and limitations</summary>

##### Corpus and evaluation protocol

The prospective public core covers VCS patches, largest tracked Rust files, recursive declaration listings, 40-commit terminal logs, and GitHub pull-list responses. Exact and structural duplicates against earlier private corpora were excluded. Public changed basenames were preserved as sanitized metadata so suggestion labels remained observable. Five cases per category were selected by content SHA-256, producing 25 core cases.

The remote extension was fixed before inspecting output content. It reconstructed completed `exec_command` and `write_stdin` sessions from 2.73 GB of authorized Codex logs, streamed 683 category candidates through an in-memory sanitizer, retained 289 eligible unique responses, and selected cases by outcome, size bucket, session diversity, and content hash. The final five-case cap retained three successful and two failed builds, three successful and two failed tests, four setup/install responses, and one static-analysis response. The server contained no LaTeX response above the 2 KiB threshold.

LaTeX coverage therefore uses arXiv sources compiled inside an isolated Linux Docker Sandbox with TeX Live and shell escape disabled. Candidate papers came from a binary-seeded shuffle of the recent `cs.LG` API pool. Repeated transport interruptions caused the live recent-entry pool to drift, so the five already downloaded public identifiers were frozen before corpus persistence or PIRA evaluation. One paper compiled successfully; its fresh source also produced a controlled undefined-command failure. Three additional papers contributed natural compilation failures, yielding one pass and four failures. Raw paper sources were disposable and were not committed.

Each suite's output-quality labels were fixed during the original holdout evaluation and were not revised for 1.4.0. The current routing thresholds and common-form filtering were fixed from live use before the selected fixtures were replayed. Every call used an identical raw fixture-emitter baseline; overhead is `wrapped wall time - raw-operation wall time`. Retained state includes captures, indexes, and event history but excludes installed binaries, runtimes, and the eight exact-replayed outputs that deliberately created no capture.

| Held-out category | Cases | Outcomes | Summary / exact | Context reduction |
|---|---:|---:|---:|---:|
| File reads | 5 | 5 success | 5 / 0 | 99.2% |
| GitHub pull retrieval | 5 | 5 success | 5 / 0 | 99.4% |
| Search and listing | 5 | 5 success | 5 / 0 | 98.9% |
| Terminal logs | 5 | 5 success | 5 / 0 | 95.8% |
| Version-control diffs | 5 | 5 success | 4 / 1 | 77.9% |
| Builds | 5 | 3 success, 2 failure | 1 / 4 | 17.5% |
| Test runs | 5 | 3 success, 2 failure | 4 / 1 | 81.9% |
| Setup and installation | 4 | 4 success | 4 / 0 | 78.7% |
| Static analysis | 1 | 1 success | 1 / 0 | 92.5% |
| LaTeX compilation | 5 | 1 success, 4 failure | 3 / 2 | 89.2% |

##### Context Mode comparison on the final corpus

Context Mode 1.0.169 was installed inside an isolated Linux Docker Sandbox and rerun without errors on the exact final 45 sanitized fixtures. Generic passthrough used one persistent server, `ctx_execute_file`, the same category-level intent as PIRA, and JavaScript that printed each fixture while preserving its recorded exit status. Its direct Node emitter produced the same bytes and exit status as its raw baseline, so Docker startup and server initialization are excluded from overhead. It returned 71,621 bytes, classified all 45 statuses correctly, and immediately exposed 9/13 labeled outcomes: 7/8 failure markers and 2/5 changed basenames.

`ctx_index` used a separate persistent server and returned 7,843 bytes of indexing receipts. It exposed none of the 13 labels immediately, while exact content remained available through later search. Indexing has no equivalent raw operation, so no synthetic latency overhead is reported. Both Context Mode storage figures include its SQLite FTS5 retrieval state after shutdown; installed packages are excluded.

Generic passthrough is the closest automatic wrapper-level comparison, not Context Mode's recommended workflow. Context Mode normally asks the model to run task-specific analysis code and return only the derived answer. Its [published benchmark](https://github.com/mksglu/context-mode/blob/main/BENCHMARK.md) reports 98% reduction for task-specific execution, 82% for exact index-plus-search retrieval, and 96% overall. Returned-context measurements here count UTF-8 bytes rather than tokenizer-specific tokens, and immediate visibility does not measure evidence recoverable by later search.

##### Limitations

This remains a private implementation benchmark on one arm64 macOS evaluation host, not a universal performance claim. The remote suite is genuinely unseen and post-freeze imported, but its logs predate the freeze and are therefore not prospective outputs. Setup/install and static-analysis coverage remains below the five-case cap because no more eligible unique remote responses were available. Failure markers measure visibility of broad outcome evidence rather than complete diagnostic usefulness. arXiv selection required baseline build availability and includes one intentionally mutated source. Privacy sanitation changes path separators in LaTeX logs. Binary, non-UTF-8, and interactive-terminal behavior are covered by functional tests rather than this corpus. Web-search returns remain excluded because Codex built-in web output is not directly captured by the local command wrapper.

</details>

</details>

### `pira_dec`: structured decision memory

Projects often lose the reason behind a choice. `pira_dec` keeps the context, serious alternatives, selected option, decision-maker, and time in one searchable record.

<details>
<summary>Technical behavior, storage, and concurrency</summary>

Each record contains an ID, UTC timestamp, short context, ordered alternatives, the selected option, and one decision-maker: human or agent. Human authorization takes precedence. Saved records are not edited in place.

The tool can add a decision, show one full record, list recent selected decisions concisely, search individual fields with regular expressions, export a selected time range as a polished standalone HTML report, or explicitly forget one exact record. Lists, searches, and exports return newer records first. Time ranges use inclusive `--since` and exclusive `--until` bounds.

HTML exports include full context, alternatives, selected choices, makers, timestamps, a compact index, responsive dark/light styling, and print support. They contain no scripts, network assets, or workspace path, and all stored text is escaped. Export creates a new file without overwriting an existing one.

Records live in private per-user application data and are separated by workspace. Multiple agents can write safely at the same time. Readers see only complete records, although a decision saved during a read may not appear until the next read. Invalid unrelated records are reported and skipped. No SQL database, daemon, network service, or repository-local metadata is required.

Public source is under `tools/src/pira_dec`. Build it with `cargo build --manifest-path tools/Cargo.toml -p pira_dec --release`, then run `pira_dec --help` or command-specific help for exact usage.

</details>

### `pira_nav`: lightweight repository navigation

Reading an entire repository is slow and context-heavy. `pira_nav` provides portable text search plus bounded code structure, structured-document paths, file relationships, and optional language-server semantics in one read-only native executable. Run `pira_nav --help` for the command chooser and `pira_nav COMMAND --help` for exact syntax.

<details>
<summary>Technical behavior, language support, security, and validation</summary>

#### Command surface

- `search` scans ordinary unignored UTF-8 repository text. Literals are the default and may be stated with `-F`; bounded Rust regular expressions, case-insensitive and Unicode half-word matching, multiple patterns, snippets, matching-file rows, and exact matching-line counts are available. Repeated patterns and up to 64 deduplicated file or directory scopes share one traversal. Missing peers in a multi-scope search are reported as incomplete while valid peers continue; a lone missing target remains an error. Every pattern is ranked independently so declaration-like production matches precede test/generated peers, while snippets remain query-balanced, capped at eight ranked lines per pattern and 8 KiB of rendered source blocks by default, and report exact shown/omitted counts. `--max-per-query` and `--max-bytes` expand deliberately, while `--owners` optionally adds enclosing clean declarations. `--max-results` aliases `--max-items`. Binary, oversized, non-UTF-8, and unreadable files are reported as aggregate completeness counts without routine per-file noise.
- `map`, `symbols` (`declaration`/`declarations` aliases), `outline`, and `show` narrow repository shape, code declarations, structured-document paths, Markdown sections, and exact source. A default map shows at most 20 balanced root-relative code/document rows and 16 landmarks balanced by kind, ranks production paths first, and counts but skips recognizable fixture/corpus subtrees; `--max-depth`/`--depth` bounds traversal. Outline output defaults to 64 items across the invocation, while `--depth 0` limits it to top-level items. Markdown outlines render local heading titles beneath indented ancestors instead of repeating the full heading path on every row; matching and `show` continue to use qualified paths. `show FILE` reads full UTF-8 files and accepts mixed bare/range/item batches; a trailing `--head N` or `--tail N` limits only the preceding bare file, including within a batch. Exact line ranges and windows require no language inference. For orientation across ultra-long lines, `show --glance` adds line numbers and shows at most the first 160 UTF-8-safe source bytes per physical line with explicit clipping metadata. Clean code uses bundled parsers; syntax-dirty code uses a conventional language server discovered on `PATH` or selected with `--lsp`. Native documents expose JSON/JSONC/YAML/TOML keys, indices, references, and tables or hierarchical Markdown headings rather than code semantics.
- `imports`, `dependents`, and `deps` expose conservative syntax-level local file relationships, including common Python absolute/relative modules, Rust module/`crate::` paths, Java/Kotlin package paths within nested source roots, and Lean 4 module-header imports. Dependency targets resolve from the current directory first and then `--root`, and must remain inside that root. Output rows default to 128 and are bounded by `--max-items`; dependency depth accepts 0 through 256. Results distinguish local, external, unresolved, ambiguous, blocked, failed, and omitted work rather than claiming a complete build graph.
- `definition`, `implementation`, `type-definition`, `references`, `callers`, `callees`, `supertypes`, `subtypes`, and `hover` use a caller-installed language server. Targets may be one-based `FILE:LINE:COLUMN`, unique `FILE::QUALIFIED-NAME`, or freshness-checked selectors. Valid peers continue when another target fails preparation or execution. `query` batches ordered mixed semantic requests and shares invocation-local server and document state.
- `languages` lists 24 compiled code languages with the discovered conventional server name or `missing`, plus five native document formats.

Language is normally inferred from the file suffix or supported shebang; `--language` handles ambiguous paths or restricts a scan, and `--` ends option parsing before positional paths beginning with `-`. Bundled code support covers Python, Rust, Java, C, C++, CUDA, Bash, Go, JavaScript, TypeScript/TSX, C#, PowerShell, PHP, Kotlin, Lua, HCL/Terraform, R, Ruby, Swift, Scala, Dart, Elixir, Julia, and Lean 4; bundled document support covers JSON, JSONC, YAML, TOML, and Markdown. Lean's dynamically extended syntax falls back to its official `lake serve`/`lean --server` LSP for structural and semantic navigation, while module-header imports remain available without a server. Ordinary readable UTF-8 text remains searchable and exactly range-readable without a parser. No language runtime, daemon, persistent index, project initialization, or network access is required for native work. Semantic accuracy and startup cost depend on the caller's language server and project configuration.

#### Relationship to existing tools

ast-outline demonstrates compact declarations and name-based retrieval; Grove demonstrates repository maps and stable broad-to-narrow identities; ripgrep/grep-class tools demonstrate fast ignored-aware lexical discovery. `pira_nav` combines the common agent-facing subset without a runtime download, persistent database, or required host search utility, then uses the standard LSP protocol when IDE semantics are needed. It deliberately leaves PCRE-only or multiline search, archive inspection, symlink traversal, replacement, diagnostics, rename, formatting, code actions, and build-system graphs to specialized tools.

#### Read-only and security boundary

Repository code is read and parsed but never edited, built, imported, or executed. Directory discovery honors ignore and hidden rules and does not follow symlinked directories; explicit search operands reject symlink components, and local dependency targets cannot escape the selected root. Source files, syntax, patterns, protocol messages, server output, results, and rendered bytes are bounded.

Source and hover text is framed as untrusted data, and unsafe terminal controls are escaped. A conservative English-keyword heuristic adds one short warning when rendered content resembles prompt injection; it never redacts, reorders, or expands source and is not a general multilingual classifier. The LSP client rejects `workspace/applyEdit`. A selected or `PATH`-discovered language server is still an external executable and may create its own caches, so its executable configuration remains part of the caller's trust boundary. Each LSP request is bounded to two minutes and timed-out server process trees are terminated; callers may still cancel earlier.

#### Validation

The correctness corpus contains 86 real and synthetic code/document files: 79 produce clean native structure and seven intentionally dirty code files are rejected by `--native` and routed to an LSP when available. It contains 1,125 structural targets with 1,125/1,125 location and freshness-selector round trips plus 74/74 curated essential target checks. Current validation also includes 36 functional black-box tests, 12 inert security tests, 37 Rust unit tests, strict Clippy, and 49 assertion-checked performance tasks spanning every command family and all 28 code/document formats. These are implementation tests, not an agentic evaluation.

#### Performance

The table reports complete subprocess calls for the optimized 0.8.0 macOS arm64 binary on an Apple M1 Pro. Each row uses five warmups and 40 measured calls over committed fixtures. Semantic rows use a deterministic minimal Python LSP server to measure PIRA's protocol path; they are not production-language-server latency estimates.

| Operation | Median latency | Peak RSS | Output |
|---|---:|---:|---:|
| `search` snippets | 4.037 ms | 4.4 MiB | 2,272 B |
| `search --files-with-matches` | 3.903 ms | 4.4 MiB | 128 B |
| `search --count` | 4.026 ms | 4.4 MiB | 164 B |
| Python `map` | 4.954 ms | 5.1 MiB | 467 B |
| Python `symbols` | 4.574 ms | 5.1 MiB | 588 B |
| Python `outline` | 5.019 ms | 3.5 MiB | 1,650 B |
| Python `show` | 5.590 ms | 3.6 MiB | 3,312 B |
| Document `outline` (five formats) | 3.016–3.279 ms | 2.1–2.6 MiB | 249–806 B |
| Python `imports` | 3.094 ms | 2.8 MiB | 423 B |
| Python `dependents` | 4.206 ms | 5.0 MiB | 281 B |
| Python `deps` | 4.233 ms | 5.0 MiB | 403 B |
| LSP `definition` | 32.326 ms | 17.7 MiB | 145 B |
| Three-operation LSP `query` | 33.734 ms | 18.1 MiB | 2,109 B |

Across all 49 tasks, the median of task medians was 4.206 ms; task medians ranged from 2.784 to 33.734 ms, median peak RSS was 4.2 MiB, and maximum peak RSS was 18.1 MiB. On a repository-scale default `map --native` of this development workspace, 153 navigable code/document files contained 1,681,918 bytes; the bounded 6,141-byte map reduced returned text by 99.6%, took 97.294 ms median / 103.101 ms p95 over five warmups and 20 measured calls, and used 44.5 MiB peak RSS. An untimed deliberately expanded inventory was 22,446 bytes. Broad-map fixture skipping excluded low-value fixture/corpus subtrees from structural rows; seven deliberately dirty non-skipped fixtures remained explicit failures in `--native` mode. Context reduction compares UTF-8 bytes, not model-specific tokens.

Validation used pinned real fixtures with retained upstream provenance, hashes, and adjacent licenses; fixture code was parsed but never executed. The fixtures and functional, security, correctness, and benchmark runners remain local rather than shipping in the release repository. Machine, filesystem, caches, source mix, server choice, and project configuration affect results.

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

- `AGENTS.md` — canonical always-on identity, behavior, tool, safety, memory, and module-routing policy
- `USER.md` — user-specific knowledge and working preferences; keep this private
- `modules/` — optional task-specific modules for research, coding, writing, learning, guidance, and maintenance
- `assets/scripts/` — setup and helper scripts
- `tools/crates/` and `tools/Cargo.toml` — isolated Rust packages in the shared PIRA tools workspace
- `tools/build/build_pira_ctx_platform_bins.py` — shared pinned, reproducibility-checking release builder configured for `pira_ctx`
- `tools/build/build_pira_dec_platform_bins.py` — package-isolated release entry point for `pira_dec`
- `tools/build/build_pira_nav_platform_bins.py` — package-isolated release entry point for `pira_nav`
- `.github/workflows/build-pira-tool-bundles.yml` — owner-dispatched build from `master` that tests all three tools, builds every platform twice, and publishes direct GitHub Release assets
- `tools/build/package_github_release.py` — validates build archives and produces the versioned release assets and checksum index consumed by setup
- `tools/src/pira_ctx/`, `tools/src/pira_dec/`, and `tools/src/pira_nav/` — public Rust implementations
- GitHub Releases — published platform executables; generated binaries are not stored on a second branch or in the source tree
- `PIRA_Voice/Samantha/` — default audio clips for optional Codex notifications

PIRA instructions use **Meaning-Preserving Telegraphic Compression (MPTC)**: filler and repetition are removed, but each rule keeps who acts, what is required, when it applies, its scope, and its exceptions. Safety and permission rules stay fully grammatical. The initial tracked-file pass reduced instruction size by **20.0%** and whitespace-delimited word count by **26.0%**; actual token savings depend on the tokenizer.

</details>

## Public/private split

<details>
<summary>What belongs in the public repository and what stays local</summary>

The public repository contains the shared policy framework plus tool source, reproducible build inputs, and verified release bundles. Local-only material should stay private:

- keep `USER.md` private;
- keep workspace-specific memory in local `AGENT_WORKBOOK.md` files;
- keep tool tests, benchmarks, fixtures, downloaded toolchains, and build work local;
- do not commit secrets or sensitive personal information.

</details>

## Acknowledgement and citation

If PIRA materially assists a research project, disclose that assistance where appropriate, such as in an acknowledgement, LLM-use disclosure, or reproducibility checklist, and cite this repository. Adapt the scope of assistance to what was actually used, and include the actual model/version or reasoning setting if your venue asks for that level of detail.

Suggested disclosure text:

> This paper was assisted by PIRA~\citep{pira}, a research-assistant agent powered by {the model used, such as GPT-5.5}. The assistance included [brainstorming / implementation assistance / writing polish / ...]. The authors are fully responsible for the final content.

Suggested BibTeX entry:

```bibtex
@misc{pira,
  author = {{PIRA Project}},
  title = {{PIRA}: A Research Assistant},
  year = {2026},
  howpublished = {\url{https://github.com/AlgebraLoveme/PIRA}}
}
```

PIRA should be acknowledged as tool assistance, not as scientific authorship.

## License

PIRA is available under the [Apache License 2.0](LICENSE).
