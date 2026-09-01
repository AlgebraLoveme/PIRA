# PIRA Claude Code parity protocol

## Claim under test

The primary claim is deliberately scoped and falsifiable:

> On the preregistered top-level task and continuity suite, the PIRA Claude Code
> routing guard provides policy-conformant behavior equivalent to the canonical
> PIRA Codex integration: every required on-demand module is loaded exactly before
> task work or a final answer, required dependencies are included, and the bridge
> does not broaden the host's permission policy.

This is behavioral parity for the tested PIRA contract, client versions, platform,
and scenarios. It is not a claim that Claude Code and Codex are identical products,
models, or agent harnesses.

Subagent parity and additional native platforms are extension claims. They may be
added only after their own gates pass. Otherwise, they remain explicit limitations
and do not appear in the primary claim.

## Frozen comparison basis

- Canonical policy: the repository `AGENTS.md` and referenced modules at the commit
  recorded in the result manifest.
- Claude system under test: the routing-guard plugin commit, Claude Code version,
  model alias, effort, and tool set recorded in the result manifest.
- Codex descriptive control: Codex CLI version, model, effort, sandbox, and config
  overrides recorded in the result manifest.
- Test data: synthetic policy paths, profile, modules, and project files. No real
  `USER.md`, prompt history, credentials, or transcript is committed.
- The same scenario prompt and project artifact content are used for both clients.

The canonical policy is the oracle. Codex observations are a descriptive control,
not an infallible label: a Codex policy miss is reported rather than copied into the
Claude acceptance rule.

## Preregistered core suite

The frozen matrix contains:

- two no-module tasks;
- every single-module entry;
- the required paper/explanation, paper/writing, coding/public-figure, and
  profile/guidance combinations;
- one adversarial task-data case;
- follow-up routing that adds or changes modules;
- a fresh-process resume;
- compaction followed by a fresh route.

Run the full 16-case stateless matrix twice independently for each client. Keep all
runs. Do not replace a failed run with a successful retry. Infrastructure failures
are labeled separately and rerun only after the cause and the invalidation rule are
recorded.

## Core metrics

For every stateless case, record:

1. selected modules;
2. modules whose exact synthetic content was loaded;
3. whether routing completed before the first task tool or final answer;
4. dependency expansion correctness;
5. task completion and any scenario-specific result check;
6. permission denials or permission-policy changes;
7. hook or parser errors;
8. duration, model turns, and client-reported usage fields when available.

Ordering is evaluated at the model-visible boundary. If one deterministic reader
invocation accepts several paths, it may load modules and task data in one process
only when every required module path precedes every task-data path in the declared
argument order and the reader processes arguments in that order. The model receives
the combined result only after the command completes, so this is treated as module
loading before model-visible task work. A reversed or indeterminate batch fails.

For continuity cases, also record whether new or changed modules are loaded after a
follow-up, resume, and compaction without relying on stale route state.

## Core acceptance gates

All gates are conjunctive:

1. **Routing:** both independent Claude matrix runs pass 16/16. No required module
   may be missing, and `none` may not coexist with a module.
2. **Ordering:** no task tool and no final answer occurs before route confirmation.
3. **Continuity:** follow-up, resume, and post-compaction probes pass twice without
   stale-state acceptance.
4. **Safety:** no permission rule is added or broadened; no arbitrary command is
   hidden behind an allowlisted wrapper; malformed routes are denied before task
   work; Stop continuation remains bounded.
5. **Failure recovery:** missing launcher/Python, invalid route arguments, changed
   module content, interrupted loading, and corrupt or stale state fail visibly and
   cannot be mistaken for a confirmed route.
6. **Installation:** validate, install, repeat install, list/verify, disable or
   uninstall, and rollback preserve unrelated user settings in an isolated config.
7. **Reproducibility:** a clean checkout can run deterministic tests and generate a
   machine-readable summary without real profile or session data.
8. **Comparative reporting:** every Codex control result is reported with the same
   scenario identifiers. Differences are disclosed; no failed control is silently
   removed or relabeled.

Passing these gates supports the exact primary claim above on each native platform
where the end-to-end client run was completed. POSIX launcher unit tests alone do
not support a native Linux or macOS parity claim.

## Extension gates

### Subagents

Subagent parity requires isolated state keyed by the client-provided `agent_id`, at
least two concurrent agents with different module routes, route confirmation before
each agent's first task tool, bounded SubagentStop recovery, and unchanged parent
state. A synthetic hook test is necessary but insufficient; one real Claude Code
subagent run is required.

### Additional native platforms

Each claimed platform requires a native Claude Code process, plugin load, route,
task tool, and permission-boundary observation. WSL launcher tests may support
portability evidence but are not labeled native Linux client validation unless the
Claude process itself runs inside WSL/Linux.

## Evidence package

The proposed follow-up PR must include:

- this frozen protocol;
- deterministic unit and lifecycle tests;
- the paired runner and schema;
- a compact committed result summary with client and policy versions;
- commands needed to reproduce the summaries;
- a security/privacy section and explicit non-claims;
- raw artifact hashes or reconstruction pointers when raw streams are intentionally
  excluded for privacy or size.

Any material runtime or routing-listing change after a recorded run invalidates that
run for the final claim and requires both Claude matrix repetitions again.
