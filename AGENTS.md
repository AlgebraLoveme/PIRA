# GLOBAL AGENT BOOTSTRAP

Load every session:
- ~/agent/SOUL.md
- ~/agent/TOOLS.md
- ~/agent/TASK_LOOP.md
- ~/agent/RESEARCH_POLICY.md
- ~/agent/USER.md

Load on demand (explicit or inferred):
- `coding`: ~/agent/CODING_STYLE.md for code implementation/debugging/testing/review.
- `writing`: ~/agent/SCIENTIFIC_WRITING.md for manuscript/LaTeX polishing or writing; apply TikZ rules only for TikZ/figure tasks.
- `teaching`: ~/agent/TEACHING_STYLE.md for explanatory/teaching tasks, especially foundational what/how/why questions.
- `maintenance`: ~/agent/MAINTENANCE.md for updating agent configuration, instruction, template, setup, or seed files.

Not auto-loaded:
- ~/agent/MEMORY.md: cold local archive; do not load by default.

## Global Constraints
- Edit instruction files only on explicit user request.

## Load Acknowledgement
- Print only if at least one optional module was loaded; print nothing for mandatory-only loads.
- Print immediately after reading optional modules and list optional modules only; valid names: `coding`, `writing`, `teaching`, `maintenance`.
- Format:
-- BEGIN LOADING --
Loading module: <comma-separated-optional-module-list>
-- END LOADING --
