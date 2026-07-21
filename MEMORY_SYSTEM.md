# MEMORY SYSTEM

Three workspace-scoped layers:

- **Low — `pira_ctx`:** shell command-purpose events/actions; agent-only, via tool retrieval.
- **Medium — `pira_decision`:** concluded choices, context, and serious alternatives; agent-only, via tool retrieval.
- **High — `AGENT_WORKBOOK.md`:** durable state, validated results, lessons, limitations, and reconstruction pointers; directly read by future agents and humans.

Retrieve only the smallest relevant memory when the task depends on it; never preload merely because it exists. Store no secrets, sensitive personal data, or unnecessary absolute paths.

## `pira_ctx`
- Default to current-thread history; use workspace scope only for genuinely relevant cross-thread operations.
- Rely on automatic thread detection; override thread IDs only in focused tests.
- Use `history` for prior events; use `recap` only after explicit compaction of the continuing thread.

## `pira_decision`
- Add only concluded decisions likely to guide later work—not routine actions, unresolved proposals, evidence, or transient details.
- Keep records concise and self-contained: decisive context, serious choices only, and one maker assigned by authority—`human` when the user selects or authorizes the conclusion; otherwise `agent`.
- Before revisiting an issue, search for prior/conflicting decisions; preserve conflicts rather than replacing history.

## `AGENT_WORKBOOK.md`
- Read/update only when high-level continuity matters. Read the smallest relevant section; read end-to-end only for whole-project consistency or compaction. Reading alone never triggers a write.
- Every entry must stand alone without `pira_ctx` or `pira_decision`; do not depend on or reference their records.
- Record only content that materially improves future understanding/decisions: current state, validated results, durable lessons/limitations, decision-relevant open items, and reconstruction pointers. Omit command transcripts, transient failures, and reproducible low-level details.
- Research work: capture how substantial modifications change the structured result. Preserve full raw Markdown tables when later consistency checks/reconstruction may need them.
- Do not re-read unchanged content. Compact only clearly stale/redundant material after an end-to-end read and concurrent-change check. Keep the workbook untracked by git.
