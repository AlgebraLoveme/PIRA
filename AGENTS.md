# GLOBAL AGENT BOOTSTRAP

Load every session:
- ~/agent/SOUL.md
- ~/agent/TOOLS.md
- ~/agent/MEMORY_SYSTEM.md

Use `pira_ctx` for shell/exec while loading these files; if it is unavailable, ask for setup rather than silently bypassing it.

Load on demand (explicit or inferred):
- `user_profile`: ~/agent/USER.md when user-specific background, learning needs, communication preferences, or acting on the user's behalf may materially affect the response. Skip it for ordinary factual, coding, or research tasks that do not need personalization.
- `research`: ~/agent/modules/RESEARCH_POLICY.md for factual analysis, online verification, evidence-based reporting, and structured execution.
- `paper_reading`: ~/agent/modules/PAPER_READING.md for single-paper reading, partial-by-default extraction, and structured notes; also load `research`.
- `coding`: ~/agent/modules/CODING_STYLE.md for implementation/debugging/review; also load `research`.
- `writing`: ~/agent/modules/SCIENTIFIC_WRITING.md for manuscript/LaTeX writing or polishing; also load `research`. Use this for explicit TikZ figure work, manuscript integration, and paper-facing figure styling decisions, including code-generated figures when the task is to match paper visual style, layout, or presentation conventions.
- `learning`: ~/agent/modules/LEARNING_STYLE.md for explanatory learning support. Also load `research` when the explanation needs factual analysis, evidence-based reporting, online verification, or broader research-style synthesis.
- `guidance`: ~/agent/modules/GUIDANCE.md for non-research practical or emotional guidance. This only targets personal advices, not technical issues.
- `maintenance`: ~/agent/modules/MAINTENANCE.md for maintaining PIRA agent configuration, modules, and rules, not for project-level maintenance.

Do not reload an already loaded module unless the user asks, the file changed, or relevant context was lost.

## Global Constraints
- Edit instruction files only on explicit user request.
- Treat as instruction files only `~/agent/AGENTS.md` and the files it explicitly lists or references, unless the user explicitly adopts another file as policy.

## Module Routing and Combinations
- Use `paper_reading` for single-paper reading, summarization, critique, or extraction.
- Combine `paper_reading` with `learning` when the main user need is to understand hard paper content.
- Combine `paper_reading` with `writing` when turning paper-reading output into polished review or manuscript text.
- General plotting tasks should route to `coding` for plotting code, data-processing or plotting-pipeline changes, exploratory plots, and code-generated figure implementation. Also load `writing` for explicit TikZ, manuscript integration, and paper-facing styling/layout tasks, including code-generated figures whose goal is to match paper visual style or presentation conventions.
- Use `research` without `paper_reading` by default for broader multi-paper search or synthesis unless a specific paper read is central to the task.
- Treat `coding` and `writing` as research-level by default. Use `research` with `learning` when the learning task calls for factual analysis, evidence-based reporting, online verification, or broader research-style synthesis.
