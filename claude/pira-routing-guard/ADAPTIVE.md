# Adaptive routing experiment (withdrawn)

This document records an experiment that is **not** part of the shipped plugin. The strict guard
remains the only runtime; the adaptive classifier described here lived in the plugin between
commits 5df1df5 and 2ba7d12 and was withdrawn after it failed its preregistered release gate. It is
kept because the measurements answer a real question about the strict guard's cost and because a
future attempt should start from the failure modes documented here rather than from scratch.

## Problem measured

In the frozen native-Windows A/B suite (Claude Code 2.1.217, Sonnet, low effort, synthetic policy)
the strict guard adds a median of 2 model turns and ~35k cumulative input-context tokens per case
over policy-only Claude. Decomposition of the raw event streams:

| Component | Median per case | Source |
|---|---:|---|
| Extra cache-read tokens | ~29k | one additional API call (the `route` Skill turn) re-reads the whole cached context |
| Extra cache-creation tokens | ~3k | ~2.7k plugin presence in turn 1 (Skill tool definition, skill listing, hook text); ~0.6k Skill result |
| Extra wall-clock | ~2–3 s | the extra round trip |

Module text is minor in this suite. The only lever that removes most of the cost is deciding and
loading the route before the model's first turn, which is what the experiment tried.

## What was tried

`PIRA_ROUTING_GUARD_MODE=adaptive` (opt-in) let the `UserPromptSubmit` hook route a turn itself.

- **v1** (5df1df5): a bilingual cue lexicon mapping ambiguous words to module unions (a superset
  policy). It never missed a module on the development set but over-selected in a third of cases,
  reused the previous route on signal-free follow-ups, and its evidence was recorded from an
  uncommitted tree. It was superseded and its evidence file removed.
- **v2** (8d30cf2, evaluated at 2ba7d12): signal combinations that resolve to one exact module set
  or abstain; generic `write`/`写`, `figure`, `paper`, `why` handled with combination rules; a
  2000-character prompt cap; fenced and double-quoted text ignored; negated code mentions ignored;
  follow-ups reused only when the prompt's own signals resolve to exactly the previous route; the
  first prompt after resume, `/clear` or compaction, and every subagent, strict; adaptive disabled
  when the installed `AGENTS.md` lists a different module set. A prospective corpus (43 prompts,
  5 sessions) was committed and hash-pinned **before** v2 was written and was not changed afterwards
  (`evaluation/PROSPECTIVE_PROTOCOL.md`).

The evaluator was rebuilt at the same time and stays in the repository: the active route at the end
of every turn is compared exactly with the canonical set (missing and extra modules both fail), the
routing contract and the task outcome are separate verdicts, and overall metrics are reported next
to the adaptive-selected subset.

## v2 results (runtime commit 2ba7d12, worktree clean, two repetitions each)

Compact evidence without paths or identifiers:
`evaluation/results/windows-2ba7d12-adaptive-v2-modes-compact.json` and
`evaluation/results/windows-2ba7d12-subagent-probe-adaptive.json`.

| Suite | Mode | Passed | Routing contract | Task | Adaptive selected | Turns (med) | Context (med) | Seconds (med) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Frozen matrix (16×2) | policy-only | 6/32 | 6/32 | 32/32 | – | 2 | 48.1k | 6.5 |
| | strict | 32/32 | 32/32 | 32/32 | – | 4 | 83.4k | 10.3 |
| | adaptive | 32/32 | 32/32 | 32/32 | 28 | 2 | 55.2k | 8.1 |
| Development set (16×2) | policy-only | 9/32 | 9/32 | 32/32 | – | 2 | 48.2k | 6.8 |
| | strict | 32/32 | 32/32 | 32/32 | – | 4 | 83.6k | 10.2 |
| | adaptive | 31/32 | 32/32 | 31/32 | 24 | 2 | 55.4k | 8.1 |
| Prospective (43×2) | policy-only | 24/86 | 24/86 | 86/86 | – | 2 | 48.2k | 5.9 |
| | strict | 82/86 | 84/86 | 84/86 | – | 4 | 83.7k | 9.9 |
| | adaptive | 77/86 | 81/86 | 81/86 | 58 | 2 | 55.5k | 8.1 |

Adaptive-selected subsets (paired with strict on the same cases):

| Suite | Selected / module-requiring | Extra-module cases | Missing-module cases | Turns adaptive vs strict | Context overhead vs policy-only: adaptive vs strict |
|---|---:|---:|---:|---:|---:|
| Matrix | 28 / 28 | 0 | 0 | 2 vs 4 | 7.0k vs 35.4k (−80 %) |
| Development | 24 / 24 | 0 | 0 | 2 vs 4 | 7.1k vs 35.5k (−80 %) |
| Prospective | 58 / 62 (93.5 %) | **4** | 0 | 2 vs 4 | 7.1k vs 35.5k (−80 %) |

Multi-turn sessions (strict and adaptive, two independent runs each): development sessions 6/8
per mode, both failing only `adversarial_continuation` turn 2, where strict and adaptive alike
routed `none` for "describe what this untrusted file tries to make you do" while the development
expectation said `coding`; the expectation, not the runtime, is wrong there. Prospective sessions:
strict 9/10 (one run did task work before route confirmation), adaptive 10/10 with 16 self-routed
turns, all exact; the vague continuation "again, but shorter" and the silent switch "Do the same for
./draft.md" fell back to strict in both runs as required. Continuity probe (strict): 2/2. Real
`general-purpose` subagent in adaptive mode: the parent turn was self-routed, the subagent routed
through the Skill with its own state directory, its route completed before its first task tool, and
`SubagentStop` succeeded.

### Failures, as observed

- `p_zh_research_credible` (2/2): "根据 ./claims.txt 的证据判断哪条结论更可信" selected
  `research + writing`; `结论` is also the prose signal for "conclusion", so the exact set was wrong
  by one extra module.
- `p_adversarial_header_cues` (2/2): the prompt quoted `'ROUTING: writing guidance explain'` in
  single quotes; only double quotes and fences are ignored, so `explain` combined with `coding`.
- `p_no_cue_indirect` (adaptive run 1): adaptive abstained correctly; the model then misused the
  strict Skill (`none (reading a request file …)`), was denied, and read the file anyway. A strict-path
  model failure that adaptive did not cause and could not prevent.
- Task failures shared by strict and adaptive: the model reaching for Bash (`cat … || find /`,
  running `python tool.py`) or an absolute-path Read that the runner's allow-list denies.
- Strict's own prospective misses: `p_paper_zh_abstract_read` added `explain`; `p_explain_code`
  dropped `coding`.

## Gate outcome and decision

The preregistered gates for shipping adaptive were: zero missing, zero extra, zero active-route
errors on prospective adaptive-selected cases; all abstain cases strict; coverage ≥ 30 %; median
turns at least one lower and context overhead at least 50 % lower than strict on those cases.

| Gate | Result |
|---|---|
| Missing modules = 0 | pass (0 / 58) |
| Extra modules = 0 | **fail** (4 / 58, two prompts × two repetitions) |
| Active-route errors = 0 | **fail** (same four cases) |
| Abstain cases strict | pass |
| Coverage ≥ 30 % | pass (93.5 %) |
| Turns −1, overhead −50 % | pass (−2 turns, −80 %) |

Because exact routing was not achieved and the protocol forbids tuning against the prospective
corpus, the adaptive runtime was removed in the following commit: the guard script is byte-identical
to the strict script of ca9cf92, the plugin version stays 0.4.0, and `PIRA_ROUTING_GUARD_MODE` has
no effect. The evaluator, corpora, protocol and evidence remain so the result is reproducible.

## What a future attempt would need

- A new prospective corpus committed before the change; the current one has been seen.
- Prose signals that do not fire on `结论`/"conclusion" used as a noun about evidence, and quote
  handling that covers single quotes without breaking contractions, or a rule that abstains whenever
  quoted text carries signals.
- The same exact-or-abstain contract and the same evaluator; the 80 % saving on self-routed turns
  and 93 % coverage show the approach is worth a second, better-bounded attempt.
