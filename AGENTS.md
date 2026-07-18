# GLOBAL AGENT BOOTSTRAP

Load every session:
- `~/agent/SOUL.md`
- `~/agent/TOOLS.md`
- `~/agent/MEMORY_SYSTEM.md`

Load them through `pira_ctx`; if unavailable, ask for setup rather than bypassing it.

Load on demand (explicit or inferred):
- `user_profile`: `~/agent/USER.md` when user background, learning needs, communication preferences, or acting on the user's behalf may materially affect the response. Skip ordinary factual, coding, or research tasks needing no personalization.
- `research`: `~/agent/modules/RESEARCH_POLICY.md` for factual analysis, online verification, evidence-based reporting, or structured execution.
- `paper_reading`: `~/agent/modules/PAPER_READING.md` for single-paper reading, summary, critique, or extraction; also load `research`.
- `coding`: `~/agent/modules/CODING_STYLE.md` for implementation, debugging, or review; also load `research`.
- `writing`: `~/agent/modules/SCIENTIFIC_WRITING.md` for manuscript/LaTeX writing or polishing; also load `research`.
- `learning`: `~/agent/modules/LEARNING_STYLE.md` for explanatory learning support.
- `guidance`: `~/agent/modules/GUIDANCE.md` for non-research practical or emotional guidance, not technical issues.
- `maintenance`: `~/agent/modules/MAINTENANCE.md` for PIRA configuration, module, or rule maintenance, not project maintenance.

Do not reload unchanged modules already in context unless the user asks or relevant context was lost.

## Constraints
- Edit instruction files only on explicit user request.
- Instruction files = `~/agent/AGENTS.md` plus files it explicitly references, unless the user explicitly adopts another.

## Routing
- Hard single-paper explanation → `paper_reading` + `learning`; polished review/manuscript text from a paper → `paper_reading` + `writing`.
- Broader multi-paper search/synthesis → `research` without `paper_reading`, unless one paper is central.
- General plotting, data processing, exploratory plots, or code-generated figures → `coding`. Add `writing` for explicit TikZ, manuscript integration, or paper-facing style/layout, including code-generated figures intended to match paper presentation.
- `coding` and `writing` are research-level by default. Add `research` to `learning` only for factual analysis, evidence-based reporting, online verification, or broader research synthesis.
