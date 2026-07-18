# MEMORY SYSTEM

Use three workspace-scoped layers:

- **Low-level — `pira_ctx`:** shell command-purpose events and actions; agent-only, accessed through tool-provided retrieval.
- **Medium-level — `pira_decision`:** concluded choices, their context, and serious alternatives; agent-only, accessed through tool-provided retrieval.
- **High-level — `AGENT_WORKBOOK.md`:** durable project state, validated results, lessons, limitations, and reconstruction pointers; read directly by future agents and humans.

Retrieve only the smallest relevant memory when the task depends on it; do not preload memory merely because it exists. Store no secrets, sensitive personal data, or unnecessary absolute paths.

## `pira_ctx`

- Use current-thread history by default; use workspace scope only for genuinely relevant cross-thread operations. Rely on automatic thread detection and override thread IDs only in focused tests.
- Use `history` for prior events. Use `recap` only after explicit compaction of the continuing thread.

## `pira_decision`

- Add only concluded decisions likely to guide later work, not routine actions, unresolved proposals, evidence, or transient details.
- Keep records concise and self-contained: state the decisive context, list only serious choices, and assign makers by decision authority (`human`, `agent`, or both when substantively joint).
- Before revisiting an issue, search for prior or conflicting decisions and preserve conflicts rather than replacing history.

## `AGENT_WORKBOOK.md`

- Read or update only when high-level project continuity matters. Read the smallest relevant section; read end-to-end only for whole-project consistency or compaction. Reading alone never triggers a write.
- Keep every entry self-contained and understandable without access to `pira_ctx` or `pira_decision`; do not depend on or reference their records.
- Record only information that would materially improve future understanding or decisions. Keep current state, validated results, durable lessons or limitations, decision-relevant open items, and useful reconstruction pointers; omit command transcripts, transient failures, and reproducible low-level details.
- In research work, capture how substantial modifications change the structured result. Preserve full raw Markdown tables when later consistency checks or reconstruction may need them.
- Do not re-read unchanged content. Compact only clearly stale or redundant material after an end-to-end read and concurrent-change check. Keep the workbook untracked by git.
