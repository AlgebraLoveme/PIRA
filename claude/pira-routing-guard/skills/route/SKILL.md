---
name: route
description: >-
  Mandatory PIRA turn router. Before any task tool or answer, invoke with
  space-separated modules: user_profile for material personalization; research
  for factual or online verification; paper_reading for one paper; coding for
  implementation, debugging, or review; writing for polished technical prose;
  public_figure for public figures; explain for explanation; guidance for
  practical or emotional support; maintenance for PIRA maintenance; none only
  when no module applies.
when_to_use: >-
  Use at the start of every user turn. Combine modules when needed.
  paper_reading, coding, writing, and public_figure imply research. A hard paper
  explanation uses paper_reading explain; polished paper prose uses
  paper_reading writing.
user-invocable: false
argument-hint: "[module ... | none]"
allowed-tools:
  - "Bash(*pira-routing-guard/*/run-routing-guard.sh *)"
---

# PIRA turn route

Selected modules: `$ARGUMENTS`

## Canonical module context

!`"${CLAUDE_SKILL_DIR}/../../scripts/run-routing-guard.sh" load "${CLAUDE_SESSION_ID}" "$ARGUMENTS"`

The content above was loaded exactly from the canonical PIRA tree. Apply it to the current user turn.
For this opt-in pilot, the dynamic-context helper is the direct file-reading mechanism required by
PIRA routing; do not issue duplicate Claude Read calls for the selected modules.

The routing guard validates the names and automatically adds canonical dependencies:

- `paper_reading`, `coding`, `writing`, and `public_figure` add `research`.
- `none` must be the only argument.

The loader omits modules already loaded and unchanged in this session. After context compaction, the guard requires a fresh route so the full selected context can be restored.
