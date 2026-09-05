# PIRA routing in Claude Code

## The problem

PIRA's policy asks the agent to read the on-demand module files (`coding`, `writing`,
`paper_reading`, …) before it starts a task. Codex follows that instruction from `AGENTS.md`. Claude
Code does not: with the policy imported through `CLAUDE.md` alone, the exact module set was loaded
before task work in 5–6 of 16 cases on a frozen synthetic matrix, and a stronger model or higher
effort did not change that (Sonnet 5 low/high, Opus 4.8, Opus 5 low/medium/high all landed at
5–7 of 16). The failure is not capability: task completion was 100 % in every run and the same models
choose the right modules once reminded. Claude Code treats project-level rules as background and reads
the task first.

## The fix shipped here

The installer adds three marked hook entries to the user's `~/.claude/settings.json`, one each for
`SessionStart`, `UserPromptSubmit` and `SubagentStart`. Each runs one `echo` of the same sentence:

> PIRA routing: before any project file read, command, or answer, Read the exact PIRA module files the
> project policy requires for this task, including their canonical dependencies. Do not re-read
> modules already loaded in this session. If no module applies, answer directly.

Claude Code appends a hook's plain stdout to the model context for these events, so the reminder
appears next to every prompt (and again after resume, `/clear` or compaction). There is no script, no
state, no tool interception and no extra model turn beyond the module reads themselves. The entries
are recognised by their `echo PIRA routing:` prefix; `--uninstall` removes only those, `--verify`
checks them, and `--skip-routing-hooks` leaves `settings.json` untouched.

## Why a reminder and not enforcement

Measured with Opus 5 (low effort) on a frozen 16-case matrix, a 43-prompt corpus written before any
of these designs existed, and 5 multi-turn sessions (task switch, vague continuation, new file, Chinese),
scored by an exact contract (canonical module set loaded before the first task tool or answer):

| Design | Matrix | Unseen corpus | Multi-turn sessions | Model turns (median) |
|---|---:|---:|---:|---:|
| `CLAUDE.md` only | 5/16 | 24/86 | – | 2 |
| This reminder (one `echo`, two reps) | 29/32 | 80/86 | 10/10 | 3 |
| Reminder + tool gate + dependency injection (plugin) | 32/32 | 78/86 | 10/10 | 3 |
| Strict guard: explicit `route` Skill, verified and enforced (plugin) | 30/32 | 84/86 (Sonnet) | 10/10 | 4 |

The gate never fired in 118 cases and 26 turns; every light variant misses the same few prompts, all
of them judgement calls (`explain` added or used alone, `guidance` paired with `user_profile`). Only a
declared, checked route catches those, at the price of a model round trip per prompt and a plugin with
a Skill, state files and evaluation harness. The reminder recovers most of the value with one line.

Known limits: evidence is native Windows, Claude Code 2.1.217, mostly Opus 5 low, one to two
repetitions per cell, sessions of 2–3 turns; long sessions between compactions and the `SubagentStart`
entry were not measured separately.

## The strict guard and the full record

The enforcing plugin, its adaptive-classifier experiment, the evaluation runners, the frozen corpora
and the per-case evidence are kept on the fork branch `claude-routing-guard-strict` (commit `ee99c1f`,
`claude/pira-routing-guard/`), not in this branch. Its `evaluation/EVIDENCE.md` and `ADAPTIVE.md`
hold the complete write-up; the compact result files there carry per-case routes, verdicts, token
usage and artifact hashes without paths or identifiers.
