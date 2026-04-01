# GLOBAL AGENT BOOTSTRAP

Load every session:
- ~/agent/SOUL.md
- ~/agent/TOOLS.md
- ~/agent/USER.md

Load on demand (explicit or inferred):
- `research`: ~/agent/modules/RESEARCH_POLICY.md for research, factual analysis, online verification, and evidence-based reporting.
- `task_loop`: ~/agent/modules/TASK_LOOP.md for structured execution loops when research, coding, or other multi-step work benefits from explicit stepwise control.
- `coding`: ~/agent/modules/CODING_STYLE.md for code implementation/debugging/testing/review.
- `writing`: ~/agent/modules/SCIENTIFIC_WRITING.md for manuscript/LaTeX polishing or writing; apply TikZ rules only for TikZ/figure tasks.
- `teaching`: ~/agent/modules/TEACHING_STYLE.md for explanatory/teaching tasks, especially foundational what/how/why questions.
- `guidance`: ~/agent/modules/GUIDANCE.md for non-research-oriented consulting such as life plans, future jobs, life decisions, emotional support, and practical personal guidance; default to this module for non-research contents.
- `maintenance`: ~/agent/modules/MAINTENANCE.md for updating agent configuration, instruction, template, setup, or seed files.

Not auto-loaded:
- ~/agent/MEMORY.md: cold local archive; do not load by default.

## Global Constraints
- Edit instruction files only on explicit user request.
- For non-research contents, default to `guidance`; load `research` and `task_loop` only when the task is research-oriented or clearly benefits from structured execution.

## Load Acknowledgement
- Print only if at least one optional module was loaded; print nothing for mandatory-only loads.
- Print immediately after reading optional modules and list optional modules only; valid names: `research`, `task_loop`, `coding`, `writing`, `teaching`, `guidance`, `maintenance`.
- Format:
-- BEGIN LOADING --
Loading module: <comma-separated-optional-module-list>
-- END LOADING --
