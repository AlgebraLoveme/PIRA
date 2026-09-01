# PIRA Claude routing guard pilot

This opt-in Claude Code plugin tests one question: can lifecycle hooks make PIRA's existing on-demand module routing reliable without copying the canonical policy tree?

The pilot is not part of normal setup. Load it only for a bounded test:

```text
claude --plugin-dir ./claude/pira-routing-guard
```

For opt-in personal daily use, install it through Claude Code's native marketplace flow from the
repository root. This changes only user-scoped Claude plugin configuration; it does not broaden
permissions or copy PIRA's canonical modules into the plugin:

```text
claude plugin marketplace add ./ --scope user
claude plugin install pira-routing-guard@pira --scope user
claude plugin list --json
```

`plugin list --json` should show `pira-routing-guard@pira` as enabled. Restart Claude Code, or run
`/reload-plugins` in an existing session, before relying on the guard. Re-running `plugin install` is
safe. To roll back the personal installation:

```text
claude plugin uninstall pira-routing-guard@pira --scope user
claude plugin marketplace remove pira
```

Removing the `pira` marketplace also removes plugins installed from it. Do not use that second command
if a future PIRA marketplace contains another plugin you want to keep.

It adds one model-only `pira-routing-guard:route` skill and hooks that require a valid route before top-level task tools or a final answer. While the pilot is loaded, the skill's dynamic-context helper is the direct file-reading mechanism for PIRA routing: it injects selected files exactly from `~/agent`, replacing only the normal model-issued Claude Read step. It neither copies modules into the plugin nor requires Claude Read access outside the project. The plugin stores no prompt or transcript content. Runtime state contains only a hash of the session ID, selected module names, tool-use IDs, and module hashes under the platform temporary directory.

The guard deliberately ignores subagent hook events in this first pilot. It also fails visibly but open when its Python launcher or script is unavailable, so an experimental installation cannot make Claude Code unusable. The pilot requires skill shell execution; if `disableSkillShellExecution` is enabled, the helper cannot confirm the route. A Stop hook requests at most one automatic retry per turn, preventing a broken pilot from creating an unbounded model loop.

Run the deterministic checks with:

```text
python claude/pira-routing-guard/tests/test_pira_routing_guard.py
python claude/pira-routing-guard/evaluation/test_run_matrix.py
python claude/pira-routing-guard/evaluation/test_run_continuity.py
```

The synthetic Sonnet evaluation matrix is opt-in and keeps raw event streams under a caller-selected
or platform-temporary artifact directory. It substitutes a tiny synthetic `PIRA_AGENT_DIR`, so routing
coverage does not disclose a real user profile and does not repeatedly inject the full policy modules.
Routing-only runs expose just `Skill`, `Bash`, and `Read` and ignore external MCP configuration to reduce
unrelated context; pass `--tools default` when a broader capability benchmark is intended:

```text
python claude/pira-routing-guard/evaluation/run_matrix.py --model sonnet --effort low
```

The runner records Claude CLI's `total_cost_usd` field as a local API-equivalent estimate for comparing
context overhead. It is not authoritative billing and is not an extra charge when Claude Code is using
included subscription authentication; those runs consume plan usage quota instead.

The separate `run_continuity.py` probe deliberately persists one synthetic session so it can verify a
fresh Claude Code process with `--resume`. Use it only when writing synthetic session history under the
active Claude configuration directory is acceptable.
