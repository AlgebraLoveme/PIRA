# SCIENTIFIC_WRITING

## Requests
- **Polish or proofread existing text**
  - **Done when:** the requested text is clearer and more concise while preserving its technical meaning, author voice, relative emphasis, uncertainty, notation, citations, and requested scope.
  - **Workflow:** infer the passage's purpose and audience from available context → identify the highest-value issues → make the smallest sufficient edits → audit grammatical agreement and every changed claim, qualifier, symbol, and citation.
  - A conservative proofread changes only genuine errors or clear friction. Do not rewrite an already effective sentence merely to impose a preferred style. Preserve paragraph order and relative emphasis unless coherence clearly improves. Section-level compaction or reordering must not silently remove claims, concessions, limitations, or decision-relevant reviewer praise; surface any meaning-sensitive deletion or emphasis shift separately.
- **Draft or restructure scientific text**
  - **Done when:** the piece serves its section-level purpose, every substantive claim is supported by supplied evidence or clearly marked as needing support, and the reader can follow the claim-evidence progression.
  - **Workflow:** deliverable, audience, and constraints → claim/evidence inventory → purpose-driven order → draft → claim, notation, and citation audit.
  - Match structure to function rather than a universal template. Add future-work statements only when requested or already present in the source. If essential evidence or intent is missing, expose the smallest consequential gap instead of filling it with plausible prose.
- **Write a rebuttal or response letter**
  - **Done when:** each concern receives a direct, factual, reviewer-usable answer that distinguishes existing evidence, clarification, manuscript changes, commitments, and remaining limitations.
  - **Workflow:** concern → direct answer → decisive evidence or reasoning → concrete change or limitation. Acknowledge praise only when it helps orient the response; avoid generic gratitude, defensiveness, vague reassurance, and unsupported promises.
- **Write explanatory or public-facing technical prose**
  - **Done when:** the target reader can identify the practical problem, operative idea, evidence, scope, and relevant limitation or next action without domain-specific clutter.
  - **Workflow:** reader question → necessary context → core mechanism or argument → concrete evidence → limitation or action only when useful.
  - Organize around reader questions, not a feature inventory. Give each section a distinct purpose and remove sections that only repeat surrounding material or offer generic advice. Define only needed jargon; use measurements and examples when they materially ground the claim. Identify the speaker whenever perspective or quotation status could be ambiguous.

Use the applicable request workflow plus the shared rules below.

## Shared Method
- Infer venue, audience, and desired intervention from the request and surrounding document. Ask one clarification only when the answer would materially change claims, structure, or voice; otherwise state a reasonable assumption and proceed.
- Prefer direct claims, coherent paragraph progression, and concrete logical connections. Remove filler, repetition, and metacommentary, but retain decision-relevant concessions, limitations, and evidence.
- Preserve established terminology, notation, equation and definition semantics, citation style, headings, and document conventions. For LaTeX prose, default to `\cref` for cross-references, `\citet` for textual citations, and `\citep` for parenthetical citations; when the existing source or venue uses another convention, follow it consistently instead.
- Default to present tense, active voice, and clear `we`; follow an established alternative voice or venue requirement when present.
- When drafting or materially polishing academic prose, avoid semicolons, em dashes, parenthetical asides, formulaic contrast constructions, and long or compound sentences unless they are the clearest way to preserve precision or flow. Prefer sentence splits, short direct affirmative phrasing, commas, or explicit connectors. Preserve semicolons required by math, code, or notation syntax. In conservative proofreading, retain an intentional existing construction when it is clear and consistent with the author's voice.
- Expand an acronym on first use in each section when needed, then use it consistently. Define unavoidable task-specific terms near first use and keep terminology consistent.
- Flag logic, evidence, or exposition gaps and propose the smallest adequate fix. Final prose must not contain ambiguous notation, undefined symbols, unexplained task-specific terms, or an obvious audience mismatch.
- Match form to information: prose for an argument, a table for compact comparison, and a visual for spatial or process structure. Do not duplicate the same content across forms without a reader need.

## Evidence and Precision
- Never introduce unsupported claims, results, citations, novelty, validation status, or implementation status. Distinguish evidence from interpretation and future work from completed work.
- Preserve uncertainty calibration. Replace an overclaim with the strongest supported statement and flag the issue when the correction is consequential.
- Treat modal verbs, negation, quantifiers, comparative or equality language, singular/plural distinctions, causal direction, and temporal or completion status as meaning-bearing technical content rather than surface style.
- A human reference, venue example, or style sample guides expression; it does not authorize transferring its claims, omissions, terminology, or confidence into the user's source. Reconcile any conflict in favor of the user's stated intent and evidence, or surface it when consequential.
- When a cleaner section would omit or demote technically valid material, preserve it unless the user requested compaction/restructuring or the surrounding document clearly provides a safer destination. Identify any non-obvious omission.
- For performance or behavior claims, retain the setting, comparison basis, metric, and scope needed to interpret the number.
- Edited, reformatted, or summarized remarks must not be presented as verbatim quotations.

## Default Output
- Return the requested deliverable only. Add a brief changelog only when requested, when edits are structurally substantial, or when the user needs to review non-obvious meaning-sensitive changes.
- Add open questions or risky assumptions only when they block a trustworthy final version. For a consequential ambiguity, provide at most two alternatives and recommend one.
- Do not add confidence tags, generic writing advice, or an offer for further work unless requested.

## Hard Constraints
- Never silently change a core technical claim or equation/definition semantics.
- Never remove decision-relevant evidence, concessions, or limitations merely for smoother prose.
- Never present pending validation, intended revisions, or future work as completed.
