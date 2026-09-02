# Claude Code routing-guard parity evidence

## Decision and exact claim

The current evidence supports a **scoped PIRA contract-parity claim**:

> On the preregistered 16-case top-level routing suite, repeated twice on native
> Windows, PIRA's Claude Code routing guard satisfied the same canonical module
> selection, dependency, ordering, continuity, and permission-boundary contract
> used by the Codex integration.

This means the Claude bridge is ready for an independently reviewable PR at the
personal-daily-use target. It does not mean that Claude Code and Codex are identical
products, that their models always choose identical routes, or that untested native
platforms have been validated.

## Frozen paired result

- Runtime/policy commit: `67239d660950997ccf7f615e7ec7c0b6edc914eb`
- Plugin: `pira-routing-guard` `0.3.0`
- Platform: native Windows 11 AMD64
- Clients: Claude Code `2.1.217`; Codex CLI `0.150.0-alpha.8`
- Models: Claude `sonnet` alias (resolved to `claude-sonnet-5`); Codex
  `gpt-5.6-sol`; both at low effort
- Machine-readable committed result:
  [`results/windows-67239d6-compact.json`](results/windows-67239d6-compact.json)
- Full generated summary SHA-256:
  `aa0a930005c5131db9c7aa923559f1a0c9f19ec3e23bb814274ca05d5321e44b`;
  it passed `parity-summary.schema.json` validation before privacy-preserving
  compaction for Git
- Worktree at run time: clean

| Client | Repetition 1 | Repetition 2 | Combined | Median case time |
|---|---:|---:|---:|---:|
| Claude Code | 16/16 | 16/16 | **32/32** | 10.188 s |
| Codex control | 14/16 | 15/16 | **29/32** | 15.706 s |

The canonical PIRA policy was the oracle; Codex was a descriptive control rather
than a source of labels. Claude therefore passed the preregistered exact routing
gate. The observed Codex differences are retained rather than normalized away:

1. `public_figure`, repetition 1: the correct `research + public_figure` route and
   task result completed, but an initial `pira_ctx` call was denied by the read-only
   Windows sandbox; the evaluator conservatively failed the case.
2. `profile_guidance`, repetition 1: Codex loaded `user_profile` but omitted
   `guidance`.
3. `guidance`, repetition 2: Codex loaded the expected `guidance` plus an unrelated
   `user_profile` module.

These three observations prevent a claim of identical per-run model choices. They
do not weaken the narrower finding that the Claude bridge met the canonical contract
on every preregistered case. The sample is not large enough to claim model
superiority.

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

At evaluation-only commit `ca73997`, with no routing-runtime change after the frozen
paired run, two independent persisted Sonnet sessions each passed:

1. initial `coding` route: `research + coding`;
2. resumed follow-up `writing` route: `research + writing`;
3. manual `/compact`: successful `PostCompact` observation plus restored
   `PIRA routing is pending` context;
4. resumed post-compact `explain` route: fresh `explain` load.

| Run | Summary SHA-256 | Compact-stream SHA-256 | Post-compact turn SHA-256 |
|---|---|---|---|
| 1 | `cc8798e6f3d448167d269bfacf185b8ef915588a5dfba5bdac0f6f4828fa95f3` | `b1b3f251dfaa0149d977a9c40f82693c4912426f045917c3a5e360cd4b9e4eda` | `80905fdcc8a9e6e7e77999ae2beff5303ccb44bcb8f1cdb18fd9ce81a798deb0` |
| 2 | `5260e62de093d6ff69ed74430d6c8079b4516d22e379722364c4662b34d0b472` | `6877261ae2bbcf5c2b1f32ed22ada4b5a9236c11034a9fac4f186dcdef8184bd` | `d44abda6c9090d85eac0e1778a182053630c8f74519ac36fdd1e7e5d564fb04b` |

Raw streams are excluded from Git because they contain local paths and session IDs.
The runner emits the same files for independent reproduction.

## Subagent extension evidence

A real Claude Code `general-purpose` subagent run with plugin `0.3.0` completed on
Sonnet and showed:

- independent parent and subagent state directories;
- `SubagentStart` injected the pending-route instruction;
- only the validated subagent route received scoped `updatedInput`;
- `research + coding` loaded before the subagent's first `Read` of task data;
- `SubagentStop` completed successfully with no retry block;
- the parent retained its separate confirmed route state.

Successful raw-stream SHA-256:
`8664af87fbc7a780541589cfe04005a8cf993ce3dbf33a14357574365ac51a76`.
One preceding attempt reached its deliberately low `$0.30` API-equivalent guard
after the ordering checkpoint and is retained as a failed attempt; the unchanged
probe completed after raising only that guard to `$0.50`.

The `$` values are Claude Code's local API-equivalent estimates used as run caps.
With subscription authentication these runs consume plan quota; they are not
evidence of separate API billing.

## Deterministic gates

The explicit suite passed **33/33** tests at `ca73997`, covering routing state,
dependency expansion, malformed input, corrupt/stale state, changed module content,
bounded Stop/SubagentStop behavior, parent/subagent isolation, concurrent distinct
subagent routes, Windows/POSIX launchers, isolated install/reinstall/disable/rollback,
matrix parsing, continuity parsing, control isolation, and result evaluation.
`claude plugin validate claude/pira-routing-guard` also passed.

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
  --client both --repetitions 2 --artifact-root <fresh-temp-directory>

py -3 -B claude/pira-routing-guard/evaluation/run_continuity.py \
  --artifact-root <fresh-temp-directory>
```

The paired runner returns nonzero if either descriptive control has any failed case;
use the machine-readable `by_client` fields and the frozen protocol's conjunctive
gates rather than treating process exit alone as the parity decision.

## Security, privacy, and explicit non-claims

- Synthetic profiles/modules and project files replace real personal context.
- No credential, real `USER.md`, prompt history, or absolute home path is committed.
- Codex runs are read-only, ignore user config/rules, disable discovered skills, and
  record any permission denial or unexpected skill access.
- The plugin does not add a general permission rule or hide arbitrary commands
  behind an allowlisted wrapper. Its only programmatic `allow` is the validated
  route-skill call needed for scoped subagent input rewriting.
- Evidence supports native Windows only. POSIX launcher tests are portability
  evidence, not native Linux/macOS client validation.
- No claim is made about full product parity, all prompts, all models, long-horizon
  autonomous work, performance superiority, or zero operational overhead.
