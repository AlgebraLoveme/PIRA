# Adaptive routing (opt-in experiment)

## Problem

In the frozen Windows A/B suite (Claude Code 2.1.217, Sonnet, low effort) the strict guard adds a
median of 2 model turns and about 30.8k cumulative input-context tokens per case over policy-only
Claude. Decomposition of the raw `deb4495` event streams:

| Component | Median per case | Source |
|---|---:|---|
| Extra cache-read tokens | ~29.4k | One additional API call (the `route` Skill turn) re-reads the whole cached context (~27k) |
| Extra cache-creation tokens | ~3.3k | ~2.7k plugin presence in turn 1 (skill listing + hook context); ~0.6k Skill result (SKILL.md body + module text) |
| Extra output tokens | ~57 | Skill call arguments + a longer answer |
| Extra wall-clock | ~2.7 s | The extra round trip |

The module text itself is a minor part of the cost in this suite. The dominant cost is the extra model
round trip that the Skill-based route requires, so the only lever that can remove most of it is to
decide and load the route **before** the model's first turn.

## Design

`PIRA_ROUTING_GUARD_MODE=adaptive` (any other value or unset keeps `strict`, which is unchanged) adds one
deterministic step to `UserPromptSubmit`:

1. The hook reads the submitted prompt (already provided by Claude Code in the hook input; it is not
   stored) and matches it against a small bilingual cue lexicon derived from the module descriptions
   in `AGENTS.md`. Each cue maps to one module or, when a word is ambiguous between modules, to the
   union of the plausible modules. Matches are expanded with the canonical dependencies.
2. If the hook is confident (rules below), it writes the `selected` state itself, injects the exact
   module text through `additionalContext`, confirms the route, and tells the model routing is complete
   and how to add modules with the `route` Skill if it disagrees. The turn then costs the same number of
   model calls as policy-only Claude.
3. Otherwise it behaves exactly like strict: the route instruction is injected and the model must call
   the `route` Skill.

The `PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`, subagent, malformed-state and permission
logic are untouched. Adaptive only changes *who* writes the `selected` state on some turns, and it
does so through the same load/commit helpers the Skill loader uses.

## Confidence rules (all deterministic)

Adaptive selects only when every rule holds; otherwise the turn is strict.

- **Non-empty cue set.** A prompt with no cue never selects anything. `none` is never chosen
  automatically; a no-module task still routes through the Skill.
- **Bounded superset.** The expanded selection has at most `ADAPTIVE_MAX_MODULES` (4) modules.
  Beyond that the prompt is either a pasted document or genuinely multi-domain, and the model's
  judgement is worth the round trip. Loading many modules also costs cache-read tokens on every later
  turn, so a large superset is not free.
- **No hold.** `SessionStart` with `source` other than `startup`, and `PostCompact`, set a one-shot
  hold so the first prompt after a resume, `/clear`, or compaction is strict. Subagents never receive
  `UserPromptSubmit`, so they are always strict.
- **Continuation without task switch.** When the previous turn ended with a confirmed non-empty route
  `R`, a follow-up whose cues are a subset of `R` (including no cues) reuses `R` without reloading
  anything already loaded. Any cue outside `R` is treated as a task switch and goes strict. A previous
  `none` route never carries over.

## Failure boundaries and why they are acceptable

- **Over-selection** (loading a module the policy does not require) is allowed by design and is the
  price of ambiguous cues (`write` → writing + coding, `figure` → coding + public_figure). It costs
  tokens, never correctness. It is measured and reported as `extra_modules`.
- **Under-selection** (a required module without any cue) is the real risk. It is bounded by three
  layers: the cue lexicon is a superset lexicon; the injected note tells the model to add modules via
  the Skill; and every case where the lexicon is silent goes strict. Held-out prompts written before
  the lexicon was tuned measure the residual rate; any miss on them is a blocker, not a tuning input.
- **Adversarial task data** can only add cues (over-selection or the cap forcing strict). It cannot
  select `none`, cannot suppress a matched module, and cannot bypass the Skill validation on strict
  turns. File contents are never read by the classifier; only the prompt text is.
- **State corruption** follows the strict rules: non-object JSON or malformed counters are normalized,
  a corrupt state is not a previous route, so the turn is strict.
- **Loader failure** (missing module file) raises inside the hook, which fails visibly with exit 1 and
  leaves the route pending, so tools stay denied until a strict route succeeds.

## Privacy

The hook never persists the prompt or a hash of it. State gains only `source` (`adaptive`/`skill`) and
the one-shot `adaptive_hold` flag; both are counts or names, as before.

## What is not attempted

- No preloading of all modules (2.5k–3k tokens per module on every later turn).
- No new shared tool; the classifier lives in the plugin script and Codex is unaffected.
- No change to the strict Skill description. Its ~350 tokens sit in the cached system prompt once per
  session, and strict routing accuracy (32/32) depends on that wording.

## Results (native Windows, Claude Code 2.1.217, Sonnet, low effort, synthetic policy)

Runtime under test: the working tree that became the first commit on `claude-routing-guard-adaptive`
(guard script, hooks and Skill unchanged between the run and that commit). Compact evidence without
paths or identifiers: `evaluation/results/windows-ca9cf92-adaptive-modes-compact.json`.

### Frozen 16-case matrix, two repetitions

| Mode | Passed | Model turns (median) | Context tokens (median) | Seconds (median) | Route Skill calls | Adaptive selections |
|---|---:|---:|---:|---:|---:|---:|
| policy-only | 5/32 | 2 | 48.1k | 6.8 | 0 | 0 |
| strict | 32/32 | 4 | 83.4k | 10.1 | 32 | 0 |
| adaptive | 31/32 | 2 | 55.4k | 8.2 | 8 | 28 |

Paired over the 28 cases adaptive handled itself: strict adds a median of 35.3k context tokens and
2 model turns over policy-only; adaptive adds 7.2k tokens and 0 turns, a 79.7 % reduction of the
overhead. The remaining ~7k is the Skill tool definition and skill listing that any guarded session
carries, plus the injected module text.

The one adaptive failure (`research`, repetition 1) is not a routing miss: research was selected and
loaded before any tool, but the model then ran a `cat ... || find /` Bash command that the runner's
allow-list denies, instead of `Read`. It is reported as observed.

In 4 of 28 selections the model invoked the `route` Skill anyway with the same modules, giving up the
saving for that case (4 turns). No adaptive case missed a required module. 4 of 28 selections loaded
an extra module (`user_profile` prompt with "draft" → writing; `public_figure` audit → coding).

### Held-out prompts (16 cases written before the lexicon), two repetitions

| Mode | Passed | Model turns (median) | Context tokens (median) | Seconds (median) | Adaptive selections |
|---|---:|---:|---:|---:|---:|
| policy-only | 9/32 | 2 | 48.3k | 6.5 | 0 |
| strict | 31/32 | 4 | 83.7k | 10.0 | 0 |
| adaptive | 32/32 | 2 | 55.5k | 7.5 | 24 |

Paired over the 24 adaptive-handled cases the overhead reduction is 79.5 %. The strict failure is a
model routing miss on `ho_poster_script` (routed `public_figure` without `coding` once); adaptive
selected `coding public_figure` from the `.py` and `poster` cues in both repetitions. Adaptive
over-selected in 10 of 24 cases (mostly `explain` from "why/为什么", `paper_reading` from "论文",
`coding` from "写"), never under-selected, and abstained on all four prompts marked `abstain`.

### Multi-turn sessions (one `stream-json` process per session)

| Session | Strict turns / context | Adaptive turns / context | Adaptive selections | Notes |
|---|---:|---:|---:|---|
| coding → continuation → writing switch → compact → explain → resume | 21 / 349.7k | 15 / 287.8k | 2 | switch, post-compact and resume turns were strict as designed |
| Chinese writing → "再短一点" → figure-code switch | 13 / 231.1k | 7 / 170.1k | 2 | switch turn strict |
| PONG → coding task | 8 / 140.7k | 5 / 111.6k | 1 | `none` never automatic |
| coding → adversarial continuation | 9 / 170.0k | 4 / 111.1k | 2 | file content cannot influence selection |

A reused continuation costs one model turn and ~28k context under adaptive against four turns and
~57.6k under strict. Wall-clock gains were consistent in the single-turn suites (about 2 s per case)
but not in the long session, where the strict-fallback turns of the adaptive run were individually
slower; that is API latency noise on equal turn counts, not a property of the mode.

### Interpretation

Adaptive meets the stated target on the population it handles: model turns 4 → 2 and a ~80 %
cut of the context overhead, with zero missed modules across 52 selections on two prompt sets.
Costs are over-selection (about one extra module in a third of held-out selections) and an ~8 % rate
of redundant Skill calls where the model routes again despite the hook's note. The `none` category
receives no benefit by design.
