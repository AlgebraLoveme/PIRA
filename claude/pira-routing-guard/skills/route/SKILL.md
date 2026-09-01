---
name: route
description: >-
  Mandatory PIRA turn router. Before any task tool or answer, select every
  applicable module. Always add user_profile when the request explicitly asks to
  use stored preferences/background or to act on the user's behalf. Use explain
  only when teaching a concept, comparison, outcome, or non-obvious logic is a
  primary deliverable, not for an ordinary analysis or evidence conclusion. Use
  coding only when source code, implementation, debugging, or software behavior
  is itself in scope, not merely because an arbitrary file is inspected. Code
  that generates or edits a publication-, paper-, README-, documentation-,
  slide-, website-, or other public-facing figure uses the two separate arguments
  `coding public_figure` with a space, never one combined name or coding alone.
  Do not add coding when only reviewing an existing SVG or image and no
  source-code work is requested. Other modules: research for factual
  analysis, evidence-based reporting, online verification, or structured
  execution; paper_reading for one paper; writing for polished technical prose;
  public_figure for public-facing figures; guidance for practical or emotional
  support; maintenance for PIRA maintenance; none only when no module applies.
when_to_use: >-
  Dependencies: paper_reading, coding, writing, and public_figure imply research.
  A hard paper explanation uses paper_reading explain; polished paper prose uses
  paper_reading writing. Use maintenance without coding for PIRA policy,
  instruction, or configuration review unless source-code work is also required.
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
