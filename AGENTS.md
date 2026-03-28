# GLOBAL AGENT BOOTSTRAP

Load every session:
1. ~/agent/SOUL.md
2. ~/agent/TOOLS.md
3. ~/agent/TASK_LOOP.md
4. ~/agent/RESEARCH_POLICY.md
5. ~/agent/USER.md

Load on demand (explicit or inferred; examples only):
- ~/agent/CODING_STYLE.md for code implementation/debugging/testing/review.
- ~/agent/SCIENTIFIC_WRITING.md for manuscript/LaTeX polishing or writing; apply TikZ plotting rules only when the task involves TikZ/figures.
- ~/agent/TEACHING_STYLE.md for explanatory/teaching tasks, especially foundational what/how/why questions.

Not auto-loaded:
- ~/agent/MEMORY.md (cold local archive; not auto-loaded).

## Maintenance
- Edit instruction files only on explicit user request.
- Exception: `MEMORY.md` may be updated when important events create stable context; keep entries concise and non-sensitive.
- Overwrite with current policy text only (no timestamps, question IDs, changelogs, or override chains).
- After every write, check cross-file conflicts; if any exist, raise them immediately and confirm resolution with the user.

## Load Acknowledgement
- Print only when at least one optional module is loaded (print nothing for mandatory-only loads).
- Print immediately after reading optional modules, and list optional modules only.
- Format:
-- BEGIN LOADING --
Loading module: <comma-separated-optional-module-list>
-- END LOADING --
- Module names: `coding`, `writing`, `teaching`.
