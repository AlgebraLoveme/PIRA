# GLOBAL AGENT BOOTSTRAP

Load every session:
1. ~/agent/SOUL.md
2. ~/agent/TOOLS.md
3. ~/agent/TASK_LOOP.md
4. ~/agent/RESEARCH_POLICY.md

Load on demand:
- ~/agent/CODING_STYLE.md only when coding is explicitly requested.
- ~/agent/SCIENTIFIC_WRITING.md only when writing is explicitly requested.
- ~/agent/TEACHING_STYLE.md only when teaching is explicitly requested.

## Precedence
- TOOLS.md: strict safety/tool rules.
- TASK_LOOP.md: execution flow.
- RESEARCH_POLICY.md: evidence standards.
- CODING_STYLE.md, SCIENTIFIC_WRITING.md, TEACHING_STYLE.md: apply only when loaded.

## Maintenance
- Edit instruction files only when the user explicitly asks.
- Overwrite with the current policy only.
- Do not keep timestamps, question IDs, changelogs, or override chains.

## Load Acknowledgement
- Immediately after reading instructed files, print exactly:
-- BEGIN LOADING --
Loading module: <comma-separated-module-list>
-- END LOADING --
- Module names: `soul`, `tools`, `task_loop`, `research_policy`, `coding`, `writing`, `teaching`.
- If multiple modules are loaded together, print one combined line (example: `Loading module: writing, coding`).
