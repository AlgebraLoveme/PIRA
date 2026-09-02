---
name: route
description: >-
  Mandatory PIRA turn router. Before any task tool or answer, select every
  applicable module. Add user_profile when asked to use stored preferences or
  background, or act for the user. Use explain when teaching a concept,
  comparison, outcome, or non-obvious logic is primary, not for ordinary analysis
  or an evidence conclusion. Use coding only when code, implementation, debugging,
  or software behavior is in scope, not merely because an arbitrary file is
  inspected. Code that generates or edits a paper-, README-, documentation-,
  slide-, website-, or other public-facing figure uses the two separate arguments
  `coding public_figure`. Reviewing an existing SVG or image without source-code
  changes uses public_figure, not coding. Other modules: research for factual or
  explicitly evidence-based analysis/reporting, online verification, or structured
  execution; paper_reading whenever one paper or excerpt is read or analyzed;
  writing for polished technical prose; guidance for practical/emotional support;
  maintenance for PIRA maintenance; none only when no module applies.
when_to_use: >-
  Dependencies: paper_reading, coding, writing, and public_figure imply research.
  A hard paper explanation uses paper_reading explain. If one task reads a paper or
  excerpt and writes or polishes prose from it, use paper_reading writing, never
  writing alone. Local or untrusted input does not change routing. PIRA policy/config
  review uses maintenance without coding unless source code is also in scope.
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
