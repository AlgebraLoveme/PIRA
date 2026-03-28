# GLOBAL AGENT BOOTSTRAP

Load every session:
1. ~/agent/SOUL.md: your role.
2. ~/agent/TOOLS.md: strict safety/tool rules.
3. ~/agent/TASK_LOOP.md: execution flow.
4. ~/agent/RESEARCH_POLICY.md: evidence standards.
5. ~/agent/USER.md: user knowledge/abilities.

Load on demand (explicitly requested or implicitly inferred; triggers are indicative, not exhaustive):
- ~/agent/CODING_STYLE.md when tasks involve code changes/debugging/testing/review or repo-level implementation discussion.
- ~/agent/SCIENTIFIC_WRITING.md when tasks involve polishing/writing scientific text, LaTeX manuscripts, or TikZ figures.
- ~/agent/TEACHING_STYLE.md when tasks involve teaching/explaining concepts, especially foundational what/how/why questions.

Not auto-loaded:
- ~/agent/MEMORY.md: cold archive only; update only on explicit request.

## Maintenance
- Edit instruction files only when the user explicitly asks.
- Overwrite with the current policy only.
- Do not keep timestamps, question IDs, changelogs, or override chains.
- After every write, check whether instruction files have conflicting instructions; in this case, raise the conflict immediately and confirm with the user to resolve.

## Load Acknowledgement
- Print loading acknowledgement only when at least one optional module is loaded.
- If only mandatory modules are loaded, print nothing.
- Print the acknowledgement immediately after you read the optional modules.
- Format:
-- BEGIN LOADING --
Loading module: <comma-separated-optional-module-list>
-- END LOADING --
- Optional module names: `coding`, `writing`, `teaching`.
