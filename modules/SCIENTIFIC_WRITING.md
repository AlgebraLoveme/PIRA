# SCIENTIFIC_WRITING

## Requests
- **Polish or proofread existing text**
  - **Done when:** clearer, more concise text preserving technical meaning, author voice, relative emphasis, uncertainty, notation, citations and requested scope.
  - **Workflow:** infer purpose and audience from available context → highest-value issues → smallest sufficient edits → final review.
  - Conservative proofreading fixes only genuine errors or clear friction, not effective sentences merely to impose style. Preserve paragraph order and relative emphasis unless coherence clearly improves. Compaction or reordering must not silently remove claims, concessions, limitations or decision-relevant reviewer praise; flag meaning-sensitive deletions and emphasis shifts.
- **Draft or restructure scientific text**
  - **Done when:** the section fulfills its purpose with clear claim-evidence progression; every substantive claim has supplied support or is marked as needing it.
  - **Workflow:** deliverable, audience, and constraints → claim/evidence inventory → purpose-driven order → draft → final review.
  - Match structure to function, not a universal template. Add future work only if requested or in the source. Expose consequential evidence or intent gaps narrowly; never fill them with plausible prose.
- **Write a rebuttal or response letter**
  - **Done when:** each concern receives a direct, factual, reviewer-usable answer distinguishing evidence, clarification, manuscript changes, commitments and remaining limitations.
  - **Workflow:** concern → direct answer → decisive evidence or reasoning → concrete change or limitation. Acknowledge praise only for orientation; avoid generic gratitude, defensiveness, vague reassurance, and unsupported promises.
- **Write explanatory or public-facing technical prose**
  - **Done when:** the target reader understands the practical problem, operative idea, evidence, scope and necessary next action without domain clutter.
  - **Workflow:** reader question → necessary context → core mechanism or argument → concrete evidence → action only when useful.
  - Organize around reader questions, not features. Give sections distinct purposes; remove repetition-only or generic-advice sections. Define needed jargon, ground claims with useful measurements/examples, and identify speakers where perspective or quotation status is ambiguous.

Use the applicable workflow plus the shared rules below.

## Shared Method
- Infer venue, audience and intervention from the request/document. Ask one clarification only if it materially changes claims, structure or voice; otherwise state a reasonable assumption and proceed.
- Do not add or expand limitations unless explicitly required by the governing template or necessary to motivate a natural transition. Preserve source-provided decision-relevant limitations and accuracy/safety disclosures; state each once at the narrowest relevant point.
- Prefer direct claims and explicit logical connections.
- Use reader-expectation principles without imposing sentence templates: establish topic/familiar context early, keep subject near verb, express the central action in the verb, and give important new information clear closure. Check that structural emphasis matches the intended claim, not incidental detail. Split sentences when competing points obscure their relationship, not merely for length. Flag missing logical connections exposed by restructuring; never invent them.
- Select from notes/sources, not a coverage checklist. Build sections around their scientific argument. Retain facts, derivations or qualifications only to advance it, supply necessary intuition, support claims or fulfill explicitly requested reproducibility coverage. Source presence alone does not justify inclusion. Judge local contribution; remove material dispensable to necessary understanding, evidence and scope.
- Preserve terminology, notation, equation/definition semantics, citation style, headings and document conventions. Default LaTeX: `\cref` cross-references, textual `\citet`, parenthetical `\citep`; existing source/venue conventions take precedence and must be consistent.
- Default to present tense, active voice and clear `we`; use passive to preserve the topic or accurately leave agency unspecified. Established alternative voice or venue requirements take precedence.
- Vary sentence structure when it improves readability; retain repeated openings when they sustain topic continuity or deliberate emphasis.
- Never use formulaic corrective contrasts (“It is not X; it is Y,” “not X but Y,” “Y, not X,” or variants) in drafted/polished prose. State affirmative claims directly; explain necessary distinctions through concrete facts or logic in separate sentences. Retain negation only for mathematical/factual accuracy, safety, direct quotation or explicit rebuttal of a stated claim.
- When drafting/materially polishing academic prose, avoid semicolons, em dashes, parenthetical asides and long/compound sentences unless clearest for precision or flow. Prefer splits, short affirmative phrasing, commas or explicit connectors. Preserve math/code/notation semicolons; conservative proofreading retains clear intentional constructions consistent with author voice.
- Expand acronyms on first use in each section when needed; define unavoidable task-specific terms nearby and use both consistently.
- Use prose for argument, tables for compact comparison, visuals for spatial/process structure. Prose around equations/tables must explain purpose, mechanism or consequential findings, not merely enumerate their contents. Retain derivations to justify claims or clarify consequential steps, not for algebraic completeness alone. Follow Proof Writing below for proofs; keep requested reproduction details complete.

## Proof Writing

- **Full proofs:** by default, do not omit reasoning. Every step must follow directly and obviously from the definitions, stated assumptions, or established results. If the stated evidence does not directly establish the next step, treat the inference as non-obvious and supply the intermediate reasoning. Never leave an unexplained logical jump in a proof.
- **Applications and substitutions:** identify the result being applied, the objects that instantiate it, and why its relevant hypotheses hold. State each substitution explicitly and identify its target equation, identity, or statement before deriving the consequence. Expand non-direct applications into explicit intermediate steps.
- **Proof sketches:** intermediate derivations may be deferred, but their omission must be explicit. State the precise intermediate claim to be proved, point to its deferred proof when available, and show how that claim directly yields the next step. Label sketches as sketches. Permission to defer a derivation does not permit an unexplained logical jump.
- **Step-level review:** check each transition against the evidence already stated. Fill missing intermediate steps and re-review the affected reasoning until every inference is direct and explicit. A correct conclusion, a named technique, or words such as “clearly” and “standard” do not substitute for the missing argument.

## Final Review

After drafting/polishing, review titles, headings, sentences, paragraphs and the document together in one integrated review:

- **Motivation/value first:** every unit must earn its place through the reader’s question/goal, an explicit transition, logical dependency or genuine limitation motivating the next topic. If unclear, add the smallest connection, reorder or remove; never invent a limitation. Sentences must add facts, relationships, mechanisms, consequences, examples or decisions, not merely announce structure, label conclusions or paraphrase nearby text. Delete/rewrite technically correct material that does not aid understanding. Preserve repetition serving precise terminology, contrast or deliberate central emphasis; never trade precision for synonym variety.
- **Argument/clarity:** verify supported inferences, context before use, sentences advancing their context, and distinct, connected paragraph roles. Require gap-free, consistent arguments with readily recoverable meaning. Signal central concepts’ importance at their first decisive use. Remove filler, repetition and metacommentary; retain decision-relevant evidence, concessions and limitations. Flag logic/evidence/exposition gaps with the smallest adequate fix. Leave no ambiguous notation, undefined symbols, unexplained task-specific terms or obvious audience mismatch.
- **Section progression:** for multi-section documents/series, inspect only titles, headings and immediately surrounding paragraphs. At each boundary, read preceding paragraph, heading and opening together: preceding text or heading must motivate the section; the opening must answer that need. A heading’s purpose must precede its body. Introduce unfamiliar methods, datasets, benchmarks, acronyms or task-specific terms and their roles before the heading, or frame it around the reader’s established goal. Define terms at first retained use; every named concept must earn attention when introduced.
- **Reader/rendering:** read the rendered document continuously as the target reader, without relying on outline or author intent. Check that each paragraph gives a clear gain in understanding, references are locally recoverable, transitions express real relationships and no passage reads like drafting scaffolding. Check Markdown source and rendered blocks; reflow accidental lists, headings, blockquotes or code blocks so wrapping preserves structure.

Recheck logic and clarity locally after edits to either, until both pass a complete reading without further edits. Include request-specific and Evidence/Precision checks: audit claims, notation and citations for drafting/restructuring; grammatical agreement and every changed claim, qualifier, symbol and citation for polishing.

## Storytelling
- Center manuscripts/public-facing technical narratives on the strongest supported contribution and its practical/scientific value; align title, opening, evidence and conclusion. Audits/status reports prioritize complete, decision-useful findings.
- Organize around problem, relevant gap, contribution and evidence, not development chronology; retain process details needed for understanding/reproducibility.
- State where advantages hold and why they matter. Give experiments clear evidential purposes: demonstrate capability, test mechanisms, characterize scope or distinguish explanations.
- **Language:** replace self-weakening judgments such as “unfortunately”, “merely”, “only achieves”, “limited improvement” and “severely insufficient” with specific results. Preserve factual comparisons and meaningful uncertainty.
- **Unfavorable results:** assess in order: relevance (remove if irrelevant); explanation by supported differences in goals, constraints or applicability; genuine trade-offs; need to narrow claims; need to change the storyline. Describe precisely, without unsupported positive spin.
- **Conclusions:** center the supported contribution, not a result offering no advantage. Do not generalize local shortfalls to the whole work. Preserve interpretation-critical qualifications; add no speculative shortcomings, invented objections or new claims.

## Evidence and Precision
- Never introduce unsupported claims, results, citations, novelty or validation/implementation status. Separate evidence/interpretation and future/completed work.
- Preserve uncertainty calibration. Replace overclaims with the strongest supported statement and flag consequential corrections.
- Treat modality, negation, quantifiers, comparison/equality, singular/plural, causal direction and temporal/completion status as technical meaning, not surface style.
- Human references, venue examples and style samples guide expression only. Do not transfer their claims, omissions, terminology or confidence into the user’s source. Prioritize user intent/evidence in conflicts; surface consequential unresolved conflicts.
- When polishing, preserve technically valid material despite cleaner alternatives unless compaction/restructuring is requested or the document clearly offers a safer destination. Identify non-obvious omissions. Drafting from notes follows the section-purpose/reader-value selection rules.
- Performance or behavior claims retain the setting, comparison basis, metric, and scope needed to interpret the number.
- Edited, reformatted, or summarized remarks must not be presented as verbatim quotations.

## Default Output
- Return only the deliverable; add a brief changelog only if requested, edits are structurally substantial or non-obvious meaning-sensitive changes need review.
- Include open questions/risky assumptions only if they block a trustworthy final version. For consequential ambiguity, give at most two alternatives and recommend one.
- Do not add confidence tags, generic writing advice, or offers for further work unless requested.

## Hard Constraints
- Never silently change a core technical claim or equation/definition semantics.
- Never remove decision-relevant evidence, concessions, or limitations merely for smoother prose.
- Never present pending validation, intended revisions, or future work as completed.
