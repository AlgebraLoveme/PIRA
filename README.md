# PIRA for Claude Code

PIRA is a research-oriented personal agent for reasoning, coding, writing, learning, and practical problem-solving. It adds inspectable instructions and local tools to Claude Code: evidence-first work, honest uncertainty, concise explanations, and explicit safety boundaries.

This is the **`claude` branch**. Use the [master branch](https://github.com/AlgebraLoveme/PIRA/tree/master) for Codex. Keep their policy checkouts separate: Claude at `~/agent-claude`, Codex at `~/agent`. Installing or updating one policy must not switch or overwrite the other's checkout.

## Install

Requires Git, Python 3.11+, and Claude Code **v2.1.281+**. Start with a fresh Claude session after setup. No Codex installation or authentication is needed.

macOS/Linux:

```bash
git clone --branch claude https://github.com/AlgebraLoveme/PIRA.git ~/agent-claude
python3 ~/agent-claude/assets/scripts/setup_pira_claude.py --dry-run
python3 ~/agent-claude/assets/scripts/setup_pira_claude.py
python3 ~/agent-claude/assets/scripts/setup_pira_claude.py --verify
```

Windows PowerShell (Python's `py` launcher):

```powershell
git clone --branch claude https://github.com/AlgebraLoveme/PIRA.git "$HOME/agent-claude"
# Stop if cloning fails; do not replace an existing directory.
py -3 "$HOME/agent-claude/assets/scripts/setup_pira_claude.py" --dry-run
py -3 "$HOME/agent-claude/assets/scripts/setup_pira_claude.py"
py -3 "$HOME/agent-claude/assets/scripts/setup_pira_claude.py" --verify
```

Inspect existing directories and local changes before adopting them. Do not switch a Codex checkout to `claude`. Setup requires real directories (including parents), not symlink/junction aliases. Policy and module files must also be independent regular files, not symlinks or hard links. Policy/module/profile paths are explicit: production checkout paths other than `~/agent-claude` and configuration paths other than `~/.claude` are rejected. `--agent-dir` and `--claude-dir` do not relocate these policy contracts.

### What setup changes

- Installs `~/.claude/rules/pira.md` as a global user rule: a symlink to `~/agent-claude/AGENTS.md` on macOS/Linux, a checked managed copy on Windows. No `CLAUDE.md` bridge or hooks are needed. `~/.claude/AGENTS.md` is not a global rule.
- Sets the built-in Project instructions option to `claude-md-and-agents-md`, loading both project formats. Resolve contradictory project rules explicitly.
- Adds narrow `Read(~/agent-claude/modules/*.md)`, `Read(~/.claude/pira/USER.md)`, and `Read(~/agent-claude/USER.md)` allowances. These grant read access, not editing or disclosure authority.
- Creates a private `~/.claude/pira/USER.md` placeholder only when missing. Existing profiles are never replaced. Use `--user-mode keep` to leave it absent; the policy then uses an existing `~/agent-claude/USER.md` as fallback, never Codex's profile.
- Installs or refreshes four shared native tools: `pira_ctx`, `pira_dec`, `pira_nav`, and `pira_svg_check`. It does not install Team or require Codex. Use `--skip-tools` when these tools are already managed separately.

**Limited coupling:** tool executables on PATH and their configured stores are intentionally shared between clients. Updating a shared binary can affect both clients; policy isolation is not binary or store isolation. Stop tool runs before updates. Setup obtains published GitHub Release binaries and verifies their size, SHA-256, and reported version; published versions may lag source. Preserve custom tool/store configuration and version pins. For custom tool destinations or selective store migration, run `assets/scripts/setup_pira_tools.py --help` and select only the four tools above with repeated `--tool NAME`, then use the Claude adapter with `--skip-tools`. Tools-only setup without a selection also includes Codex Team.

Store defaults: macOS `~/Library/Application Support/PIRA`, Linux `$XDG_DATA_HOME/pira` (or `~/.local/share/pira`), Windows `%LOCALAPPDATA%\PIRA`, with `ctx/` and `decision/` children. Existing explicit store overrides are respected. Migration preserves originals and stops on conflicts; never delete old stores to bypass an error. Run setup directly, not inside `pira_ctx`, to avoid an enclosing capture writing after migration. Restart agents/shells when PATH or environment changes require it.

### Update, verify, uninstall

Close active PIRA sessions. Inspect local changes, confirm the checkout's branch is `claude`, then fast-forward without discarding local work:

```bash
git -C ~/agent-claude status --short
git -C ~/agent-claude branch --show-current
git -C ~/agent-claude pull --ff-only
python3 ~/agent-claude/assets/scripts/setup_pira_claude.py --user-mode keep
python3 ~/agent-claude/assets/scripts/setup_pira_claude.py --verify
```

On Windows, use the same Git commands with `"$HOME/agent-claude"`, and `py -3` instead of `python3`. Rerun setup after source updates on Windows to refresh the rule copy. Pulling Git alone does not refresh executables.

Setup preserves unrelated settings and instructions, backs up changed user files, and refuses unmanaged or changed target rules. Potentially overlapping or unrecognized Read deny/ask restrictions are refused, never silently overridden. `--verify` checks only the user settings file it manages, not project, managed, or session permission layers. Check `/context` in a fresh Claude session to confirm rule loading.

```bash
python3 ~/agent-claude/assets/scripts/setup_pira_claude.py --uninstall --dry-run
python3 ~/agent-claude/assets/scripts/setup_pira_claude.py --uninstall
```

Uninstall restores the previous project-instruction mode and removes only installer-owned allowances, the rule, and its manifest. It preserves profiles, tools, stores, and unrelated files. Caught install/uninstall errors attempt to restore all managed-file state; inspect reported paths if rollback fails. Recovery does not cover process termination, concurrent edits, or shared-tool installation.

## Migrate an existing shared-source Claude installation

Do **not** change `~/agent` or Codex configuration. Stop Claude and tool runs. Preserve private profiles and inspect local changes first.

1. Clone the `claude` branch into the separate `~/agent-claude` directory as above. Do not install over the existing owned rule.
2. From the new checkout, run `python3 ~/agent-claude/assets/scripts/setup_pira_claude.py --uninstall --dry-run`, then `--uninstall`. Uninstall uses the old `~/.claude/pira/rule_install.json` and its recorded source path, not the new default. It can remove the old owned shared-source rule even if that source is gone. If ownership/settings have changed, stop and inspect the diagnostic; never delete a conflicting rule to force adoption.
3. Run the new setup with `--user-mode keep` (and `--skip-tools` if shared binaries need no update), then `--verify`. This leaves any existing private Claude profile unchanged. If you previously relied on `~/agent/USER.md`, explicitly copy the desired preferences to `~/.claude/pira/USER.md` yourself after inspection; the installer does not read, move, or overwrite that private Codex profile.

Older snapshot/bridge installs without a rule manifest go directly to step 3. Setup removes only the recognized managed PIRA import block, preserving unrelated `CLAUDE.md` text. Custom snapshot imports require an unchanged ownership manifest; unquoted whitespace imports and edited/unowned bridges are rejected for manual review. Old snapshots under `~/.claude/pira/` remain untouched and inactive; do not remove `USER.md` when reviewing them. For an old custom configuration directory, `--uninstall --claude-dir PATH` can remove its owned rule before reinstalling at the supported defaults.

Use `py -3` and expanded `$HOME` paths for these commands in Windows PowerShell.

## Native Claude workflow

Use Claude's native Read tool for on-demand instruction modules and Bash for shell commands. `pira_ctx` retains evidence-bearing output such as tests and builds; it is not required around every native Bash command. On Windows, run PIRA tools through Claude's POSIX Bash tool, not PowerShell.

Use native subagents for substantial independent work. Pass explicit scope, edit versus review-only authority, necessary instructions/module pointers, and expected deliverables. Non-fork workers do not inherit the main conversation or previously read files; Explore and Plan skip instruction files. Keep essential safety constraints in the delegation itself when instructions cannot be loaded. Give parallel editors disjoint ownership; no separate Team scheduler is required.

Unattended `claude -p` tool use needs explicit permissions. Prefer narrow `--allowedTools` rules for the needed Bash calls; do not grant blanket Bash access merely for PIRA. Permission allowances are not authority to edit files, disclose profiles, or expand a task. PIRA safety rules are guidance, not an operating-system sandbox.

## Shared tools and source

| Tool | Purpose |
| --- | --- |
| `pira_ctx` | Retain command output and retrieve exact evidence without flooding context |
| `pira_dec` | Record concluded decisions and serious alternatives |
| `pira_nav` | Inspect source structure, text, and supported semantic relationships |
| `pira_svg_check` | Conservative SVG text-legibility warnings |

Use each tool's `--help` for commands. Shared Rust sources, release builders, setup helpers, and the Codex-only `pira_team` source remain available for upstream reuse; Claude setup does not install or invoke Team. Detailed Codex/Team usage belongs to the master branch.

`AGENTS.md` defines global behavior and routing; `modules/` contains task-specific research, coding, writing, explanation, figure, guidance, and maintenance policies. `assets/MODULE_LOADING_CASES.md` provides routing probes. Keep `USER.md`, workspace memory, credentials, and personal data private.

## Offline checks

```bash
python3 -B -m unittest -q assets.scripts.test_setup_pira assets.scripts.test_setup_pira_tools assets.scripts.test_setup_pira_claude assets.scripts.test_smoke_pira_routing
```

These deterministic fixtures cover setup safety, migration, rerun/uninstall, dual-client policy isolation, and complete-content read evidence. They make no authenticated model calls. Installer verification and parser tests do not establish live global rule loading, native subagent context, or effective session permissions. Authenticated smoke runners remain local and are not shipped.

## Acknowledgement and license

Disclose material PIRA assistance in research and cite this repository where appropriate, with the actual model/version when required. Human authors remain responsible for the work; PIRA is tool assistance, not scientific authorship.

[Apache License 2.0](LICENSE).
