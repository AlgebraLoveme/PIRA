# Claude Code routing-guard evidence

## Decision and exact claim

The current evidence supports a **scoped reliability and PIRA contract-parity
claim**:

> On the preregistered 16-case top-level routing suite, repeated twice on native
> Windows, guarded Claude Code satisfied the canonical PIRA module-selection,
> dependency, and pre-work ordering contract in 32/32 cases, versus 5/32 for the
> same Claude Code model and policy without the guard. Continuity, installed-policy,
> subagent, failure-recovery, and permission-boundary checks also passed.

This means the Claude bridge is ready for an independently reviewable PR at the
personal-daily-use target. It does not mean that Claude Code and Codex are identical
products, that every prompt benefits by the same amount, or that untested native
platforms have been validated.

## Frozen Claude A/B result

- Runtime/policy commit: `deb449567a929248b15f79cae0c733fa6b00bf6e`
- Plugin: `pira-routing-guard` `0.4.0`
- Platform: native Windows 11 AMD64
- Client: Claude Code `2.1.217`, `sonnet` alias (resolved to `claude-sonnet-5`),
  low effort
- Machine-readable committed result:
  [`results/windows-deb4495-claude-ab-compact.json`](results/windows-deb4495-claude-ab-compact.json)
- Full generated summary SHA-256:
  `58e4860fbdba5191bd7c7f2d4bb10354d7b026cd23f08b8e77e6e161cc96fa45`;
  it passed `parity-summary.schema.json` validation before privacy-preserving
  compaction for Git
- Worktree at run time: clean

| Mode | Repetition 1 | Repetition 2 | Combined | Median case time |
|---|---:|---:|---:|---:|
| Claude policy only | 2/16 | 3/16 | **5/32** | 7.032 s |
| Claude + routing guard | 16/16 | 16/16 | **32/32** | 9.816 s |

Both modes used the same executable, model, effort, prompt matrix, synthetic policy,
and task artifacts. The policy-only mode omitted the plugin and exposed only native
`Read` and `Bash`; the guarded mode added the route Skill and its narrowly allowed
loader. In the 27 policy-only failures, 25 loaded no required module and two loaded
only a partial dependency set. The only policy-only module-required pass was
`profile_guidance` in repetition 2; the four remaining passes were the two `none`
cases in both repetitions.

The result supports an incremental reliability claim for this frozen suite. It does
not isolate whether every gain comes from enforcement, the explicit routing prompt,
or their interaction, and the roughly 2.8-second median difference is descriptive,
not a general performance estimate.

## Earlier Codex descriptive control

The earlier frozen result at runtime commit `67239d6` remains committed as
[`results/windows-67239d6-compact.json`](results/windows-67239d6-compact.json).
With the same 16-case matrix repeated twice, guarded Claude passed 32/32 and the
isolated Codex control passed 29/32. It was not rerun for plugin `0.4.0`, so it is
historical cross-client context rather than the current A/B result. Its observed
Codex differences are retained rather than normalized away:

1. `public_figure`, repetition 1: the correct `research + public_figure` route and
   task result completed, but an initial `pira_ctx` call was denied by the read-only
   Windows sandbox; the evaluator conservatively failed the case.
2. `profile_guidance`, repetition 1: Codex loaded `user_profile` but omitted
   `guidance`.
3. `guidance`, repetition 2: Codex loaded the expected `guidance` plus an unrelated
   `user_profile` module.

These observations prevent a claim of identical per-run model choices. They do not
weaken the narrower finding that the Claude bridge met the canonical contract on
every preregistered case. The sample is not large enough to claim model superiority.

## Control-isolation correction

An earlier paired run was invalid because `--ignore-user-config` did not prevent
Codex from discovering and reading an installed user skill. The runner now:

- discovers local user, plugin, repository, admin, and bundled skill files;
- disables each with a highest-precedence `skills.config` CLI override;
- fails any case that still accesses an external `SKILL.md`;
- records only the disabled-skill count and a path-manifest digest.

The frozen run disabled 47 discovered skills and observed **zero** external skill
reads in all 32 Codex cases. The invalid run remains diagnosis evidence and is not
used for the parity claim. This follows Codex's documented per-skill disable and
configuration-precedence mechanisms:

- <https://learn.chatgpt.com/docs/build-skills>
- <https://learn.chatgpt.com/docs/config-file/config-basic>
- <https://learn.chatgpt.com/docs/config-file/config-reference>

## Continuity and compaction

At frozen runtime commit `deb4495`, two independent persisted Sonnet sessions each
passed:

1. initial `coding` route: `research + coding`;
2. resumed follow-up `writing` route: `research + writing`;
3. manual `/compact`: successful `PostCompact` observation plus restored
   `PIRA routing is pending` context;
4. resumed post-compact `explain` route: fresh `explain` load.

| Run | Summary SHA-256 | Compact-stream SHA-256 | Post-compact turn SHA-256 |
|---|---|---|---|
| 1 | `92af0252ae8876599a0dc67438b6b453e51bfb3ad853508961132f01fc1ede99` | `e22e12196eb757b53f1741900e0187a20df72d53c9ba0bc1941f5c3bdc8e6323` | `06ec21c5a1a3ba3fa88580e925d09cf07b83b2e3e3cd402caebc806f70052230` |
| 2 | `b87fefdcba543e18b08b4a9dd42f4a8b0877d6443e9c651dfdf1b6a9c698faa5` | `8aae74a863b5e478a6770e2c0ce9aa0f5a6e95b8c0aa8c7bf49ea4793c8998a7` | `6b82978f574945ce2c8792f81ee8ce0a5c7d37e9918e90397adbe2effcb36924` |

Raw streams are excluded from Git because they contain local paths and session IDs.
The runner emits the same files for independent reproduction.

## Subagent extension evidence

A real Claude Code `general-purpose` subagent run with plugin `0.4.0` completed on
Sonnet against the installed `~/.claude/pira` snapshot and showed:

- independent parent and subagent state directories;
- `SubagentStart` injected the pending-route instruction;
- only the validated subagent route received scoped `updatedInput`;
- `research + coding` loaded before the subagent's first `Read` of task data;
- `SubagentStop` completed successfully with no retry block;
- the parent retained its separate confirmed route state.

The subagent loaded `research + coding` before its first task `Read`, reported the
off-by-one defect, made no file change, and exited with successful `SubagentStop` and
top-level result events. Raw events remain local because they contain absolute paths
and session/agent IDs.

A separate zero-tool process verified the bounded fail-open path: it stated that
the Skill tool was unavailable, received exactly one Stop retry, repeated the final
answer, and exited successfully. This is availability evidence, not a successful
route or parity case.

The `$` values are Claude Code's local API-equivalent estimates used as run caps.
With subscription authentication these runs consume plan quota; they are not
evidence of separate API billing.

## Deterministic gates

The explicit suite passed **37/37** tests at `deb4495`, covering routing state,
dependency expansion, malformed input, corrupt/stale state, changed module content,
bounded Stop/SubagentStop behavior, parent/subagent isolation, concurrent distinct
subagent routes, Windows/POSIX launchers, isolated install/reinstall/disable/rollback,
matrix parsing, continuity parsing, policy-only observation, control isolation, and
result evaluation. `claude plugin validate claude/pira-routing-guard` also passed. A
separate isolated process with no `PIRA_POLICY_DIR` override successfully loaded
`explain` from the installed `~/.claude/pira` snapshot and completed its answer.

## Reproduction

Run from the repository root. Choose fresh temporary artifact directories so failed
runs are never overwritten.

```text
py -3 -B -m unittest \
  claude/pira-routing-guard/tests/test_pira_routing_guard.py \
  claude/pira-routing-guard/tests/test_routing_launcher.py \
  claude/pira-routing-guard/tests/test_marketplace_installation.py \
  claude/pira-routing-guard/evaluation/test_run_matrix.py \
  claude/pira-routing-guard/evaluation/test_run_continuity.py \
  claude/pira-routing-guard/evaluation/test_run_parity.py

claude plugin validate claude/pira-routing-guard

py -3 -B claude/pira-routing-guard/evaluation/run_parity.py \
  --client claude-ab --repetitions 2 --artifact-root <fresh-temp-directory>

py -3 -B claude/pira-routing-guard/evaluation/run_continuity.py \
  --artifact-root <fresh-temp-directory>
```

The paired runner returns nonzero if either A/B mode has any failed case;
use the machine-readable `by_client` fields and the frozen protocol's conjunctive
gates rather than treating process exit alone as the parity decision.

## Security, privacy, and explicit non-claims

- Synthetic profiles/modules and project files replace real personal context.
- No credential, real `USER.md`, prompt history, or absolute home path is committed.
- Policy-only and guarded Claude runs use synthetic modules and project data; the
  historical Codex runs are read-only, ignore user config/rules, disable discovered
  skills, and record any permission denial or unexpected skill access.
- The plugin does not add a general permission rule or hide arbitrary commands
  behind an allowlisted wrapper. Its only programmatic `allow` is the validated
  route-skill call needed for scoped subagent input rewriting.
- Evidence supports native Windows only. POSIX launcher tests are portability
  evidence, not native Linux/macOS client validation.
- No claim is made about full product parity, all prompts, all models, long-horizon
  autonomous work, performance superiority, or zero operational overhead.

## Adaptive routing experiment (withdrawn)

Between 5df1df5 and 2ba7d12 this branch carried an opt-in adaptive mode that routed confident
prompts from the `UserPromptSubmit` hook. Its v1 evidence (`windows-ca9cf92-adaptive-modes-compact.json`,
recorded from an uncommitted tree) was superseded and removed. The v2 runtime was evaluated on the
clean commit 2ba7d12 against the frozen matrix, the development set, the hash-pinned prospective
corpus, multi-turn sessions, the strict continuity probe and a real subagent; results are in
`results/windows-2ba7d12-adaptive-v2-modes-compact.json` and
`results/windows-2ba7d12-subagent-probe-adaptive.json` and are summarised in `../ADAPTIVE.md`.
Strict stayed at 32/32 on the matrix; adaptive cut the context overhead of self-routed cases by
about 80 % but selected an extra module on 4 of 58 prospective self-routed cases, failing the
exact-routing gate, so the adaptive runtime was removed. The shipped guard script is byte-identical
to the strict script of ca9cf92 and `PIRA_ROUTING_GUARD_MODE` has no effect.

Prospective strict observations from the same run, reported as observed: 84/86 exact routes (one
`explain` added to a paper-reading prompt, one `coding` dropped from an explain-code prompt) and
2/86 task failures from denied Bash or absolute-path Read calls.
