# Prospective validation protocol for adaptive routing

## Purpose

`development.json` (formerly `heldout.json`) was written before the first adaptive lexicon, but the
lexicon was then tuned while that file was visible, so it is a development set. The prospective
corpus in `prospective.json` and `prospective-multiturn.json` is committed **before** the v2
classifier is implemented and is never edited afterwards. `test_prospective_corpus.py` pins the
SHA-256 of both files; any later change is visible as a test failure and disqualifies the run.

## Corpus design

The prospective cases were written against the canonical routing rules in `AGENTS.md`, not against
any lexicon. They cover, in English and Chinese: cue synonyms and negatives, same-word different
sense (`code`, `test`, `paper`, `figure`), no-cue task switches, continuation mixed with a new task,
single- and multi-module tasks, prompts that must abstain, the public-figure/code boundary, the
paper-reading/paper-writing boundary, the explain/research/coding boundary, a long prompt, and
adversarial prompt text. Multi-turn sessions cover a silent switch to a new file of a different
domain, a vague continuation without a domain signal, a new task announced as a continuation, a
Chinese continuation followed by a switch, and a new file in the same domain.

Each case carries the exact canonical module set (`expected_loaded`). `expect_adaptive: "abstain"`
marks prompts the classifier must never route itself. `accept_any_route` relaxes only the strict
model's exact-route check where the canonical policy leaves the choice to judgement; it never
relaxes the abstain expectation and never relaxes the exact check on an adaptive self-route.

## Scoring

For every case and mode the runner records two independent verdicts:

- **routing contract**: the active route at the end of the turn equals `expected_loaded` exactly
  (missing and extra modules both fail), every module of that route has been loaded in the session,
  and routing completed before the first task tool or answer. When the model re-routes with the
  Skill after an adaptive selection, the final active route is what is checked.
- **task**: successful result, no permission denials, and the case's `result_regex` if any.

A case passes only when both hold. Pass counts, per-case medians (model turns, context tokens,
cache creation, cache read, output tokens, seconds) and paired deltas against policy-only are
reported for **all cases** of each mode and, separately, for the adaptive-selected subset.
Coverage is the share of module-requiring cases (`expected_loaded` non-empty, not marked abstain)
that adaptive routed itself.

## Rules

1. Run the prospective corpus only after the runtime under test is committed and the worktree is
   clean; the manifest must show `worktree_dirty: false` and `policy_commit` equal to that commit.
2. No cue, threshold, or rule may be changed in response to a prospective result. A change requires
   a new prospective corpus committed before the changed classifier is evaluated.
3. Failures are reported as observed. A rerun is allowed only for a documented infrastructure
   failure (client crash, timeout, budget cap before any model output) after its cause is recorded;
   a random model failure is never replaced.
4. Every case's route, verdicts, model turns, usage, duration, and artifact hashes are kept in the
   compact evidence; raw streams stay in the platform temporary directory.

## Release gates for adaptive

- No regression in the strict deterministic and behavioural tests.
- Prospective adaptive-selected cases: missing modules 0, extra modules 0, active-route errors 0.
- Every prospective case marked `abstain` falls back to the strict Skill route.
- Coverage of module-requiring prospective cases at least 30 %.
- On adaptive-selected cases, median model turns at least one lower than strict and context
  overhead over policy-only at least 50 % lower than strict's.
- Overall metrics for all cases are reported alongside the subset.
- Strict remains the default; adaptive remains opt-in; platforms other than native Windows remain
  unverified unless separately evidenced.
