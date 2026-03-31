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

Not auto-loaded:
- ~/agent/MEMORY.md: cold local archive; do not load by default.

## Maintenance
- Edit instruction files only on explicit user request.
- `MEMORY.md` is local-only storage for context that may be sensitive.
- If the user requests an insensitive memory update, write it to `~/agent/assets/MEMORY_INIT.md` instead of `MEMORY.md`.
- Exception: `MEMORY.md` may still be updated for important local-only context; keep entries concise and non-sensitive unless the user explicitly wants sensitive local storage.
- Overwrite with current policy text only: no timestamps, question IDs, changelogs, or override chains.
- After every write, check for cross-file conflicts; if any exist, raise them immediately and confirm resolution with the user.

## Load Acknowledgement
- Print only if at least one optional module was loaded; print nothing for mandatory-only loads.
- Print immediately after reading optional modules.
- List optional modules only; valid names: `coding`, `writing`, `teaching`.
- Format:
-- BEGIN LOADING --
Loading module: <comma-separated-optional-module-list>
-- END LOADING --
