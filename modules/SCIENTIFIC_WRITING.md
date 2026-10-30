# SCIENTIFIC_WRITING

## Requests

- **Polish or proofread existing text**
  - **Done when:** clearer, more concise text preserving technical meaning, author voice, relative emphasis, uncertainty, notation, citations and requested scope.
  - **Workflow:** infer purpose and audience from available context → highest-value issues → smallest sufficient edits.
  - Conservative proofreading fixes only genuine errors or clear friction, not effective sentences merely to impose style. Preserve paragraph order and relative emphasis unless coherence clearly improves. Compaction or reordering must not silently remove claims, concessions, limitations or decision-relevant reviewer praise; flag meaning-sensitive deletions and emphasis shifts.
- **Draft or restructure scientific text**
  - **Done when:** the section fulfills its purpose with clear claim-evidence progression; every substantive claim has supplied support or is marked as needing it.
  - **Workflow:** deliverable, audience, and constraints → claim/evidence inventory → purpose-driven order → draft.
  - Match structure to function, not a universal template. Add future work only if requested or in the source. Expose consequential evidence or intent gaps narrowly; never fill them with plausible prose.
- **Write a rebuttal or response letter**
  - **Done when:** each concern receives a direct, factual, reviewer-usable answer distinguishing evidence, clarification, manuscript changes, commitments and remaining limitations.
  - **Workflow:** concern → direct answer → decisive evidence or reasoning → concrete change or limitation. Acknowledge praise only for orientation; avoid generic gratitude, defensiveness, vague reassurance, and unsupported promises.
- **Write explanatory or public-facing technical prose**
  - **Done when:** the target reader understands the practical problem, operative idea, evidence, scope and necessary next action without domain clutter.
  - **Workflow:** reader question → necessary context → core mechanism or argument → concrete evidence → action only when useful.
  - Organize around reader questions, not features. Give sections distinct purposes; remove repetition-only or generic-advice sections. Define needed jargon, ground claims with useful measurements/examples, and identify speakers where perspective or quotation status is ambiguous.

**Additional completion gate for draft/restructure, rebuttal/response, and explanatory/public-facing prose:** all included material must be important to the intended message. Removing any passage (in every level, including contents, paragraphs, sentences and words) must cause a significant loss of meaning (include logic transitions) or understanding (include intuitions) necessary to follow the argument.

Use the applicable workflow plus the shared rules below.

## Shared Method
- **Purpose and essential content.**
  - Infer the audience and intended message from the request/document. Ask for clarification only when it materially changes claims, structure or voice; otherwise state a reasonable assumption and proceed.
  - Center manuscripts and public-facing narratives on the strongest supported contribution and its practical or scientific value; align title, opening, evidence and conclusion. Audits and status reports prioritize complete, decision-useful findings.
  - Select material for the central argument, necessary intuition or supporting evidence. Do not include details solely for reproducibility unless the user’s request or the document’s purpose clearly calls for them. Source presence or technical correctness alone does not earn inclusion. Omit self-evident qualifications and do not preempt interpretations that the text gives readers no reason to make. Decide what each sentence must help the reader understand at that point, then select and phrase facts to serve that purpose. Mere relevance, structural announcements, conclusion labels and nearby paraphrases do not earn inclusion.
- **Evidence-grounded claims without defense.**
  - Never introduce unsupported claims, results, citations, novelty or validation status. Distinguish evidence from interpretation and completed work from plans. State the strongest supported claim; omit strongly uncertain assertions rather than adding uncertainty commentary, without implying greater certainty than the evidence supports.
  - Do not weaken, qualify, reject, rewrite or supplement a user-specified claim merely because its evidence is mildly strong rather than absolute. Debate claims on evidential grounds only when concrete contrary evidence overturns them; do not change the claim by users in writing. For positive or negative findings, do not append commentary about what remains valid or unproven.
  - Retain the setting, comparison basis, metric and scope needed to interpret performance or behavior claims. For unfavorable results, assess relevance, supported explanations, genuine trade-offs, and whether the claim or storyline must change. Do not add positive spin, speculative shortcomings or invented objections, or generalize a local shortfall to the whole work. Debate/clarify this with users rather than write unfavorable claims/results into the writeup.
- **Fidelity to meaning and author intent.**
  - Never silently change core claims, equations or definitions. Treat modality, negation, quantifiers, equality/comparison, singular/plural, causal direction and temporal/completion status as technical meaning. Verify claims, notation and citations when drafting; check grammatical agreement and every changed claim, qualifier, symbol and citation when polishing.
  - Keep requested edits within scope. When polishing, preserve technically valid material unless compaction/restructuring is requested or the document clearly provides a safer destination; identify non-obvious omissions. Only keep decision-relevant evidence, concessions or limitations for internal writeup, never for public-facing writeup.
- **Argument and progression.**
  - Organize around the problem, relevant gap, contribution and evidence rather than development chronology; retain process details needed for understanding. State where advantages hold and why they matter. Give experiments explicit evidential purposes.
  - Establish context before use, give paragraphs distinct connected roles, and make references locally recoverable. Each section must answer a need established by the preceding text or its heading. Introduce unfamiliar named concepts and their roles before a heading uses them, or frame the heading around the reader’s established goal.
  - Express logical connections directly. Never invent a limitation or missing inference to supply a transition; resolve logic or exposition gaps with the smallest adequate change.
- **Explicit proof reasoning.**
  - In full proofs, do not omit reasoning by default. Every step must follow directly and obviously from definitions, stated assumptions or established results; supply intermediate reasoning wherever it does not. Check each transition and repair affected reasoning until every inference is explicit. A correct conclusion, named technique, “clearly” or “standard” cannot replace an argument.
  - For applications, identify the result, its instantiated objects and why its hypotheses hold. State each substitution and its target equation, identity or statement before deriving the consequence; expand non-direct applications into intermediate steps.
  - Label proof sketches. Deferred derivations must be explicit: state the precise intermediate claim, point to its deferred proof when available, and show how it yields the next step. Deferral does not permit unexplained logical jumps.
- **Precise terminology and representation.**
  - Preserve terminology, notation, citation style, headings and document conventions. Define symbols and unavoidable task-specific terms before use; expand acronyms on first use in each section when needed. Use them consistently rather than trading precision for synonym variety.
  - Use prose for argument, tables for compact comparison and visuals for spatial/process structure. Explain the purpose, mechanism or consequential findings of equations and tables rather than enumerate them. Retain derivations for justification or understanding, not algebraic completeness alone; keep requested reproduction details complete and follow the proof requirements above.
  - Existing source/venue conventions take precedence. Default LaTeX references are `\cref`, textual `\citet` and parenthetical `\citep`. Keep Markdown structure aligned with rendered meaning; fix accidental lists, headings, blockquotes or code blocks.
- **Direct, readable sentences.**
  - Put familiar context early, keep subject near verb, express central actions in verbs, and give important new information clear closure. Emphasize the intended claim rather than incidental detail. Split competing points when their relationship becomes obscure, not merely because a sentence is long.
  - Default to present tense, active voice and clear `we`; use passive to preserve the topic or accurately leave agency unspecified. Respect established voice and venue requirements. Vary sentences for readability; retain repetition serving topic continuity, precise terminology or deliberate emphasis.
  - Never use formulaic corrective contrasts such as “not X but Y” or “Y, not X.” State affirmative claims directly and explain necessary distinctions through concrete facts or logic in separate sentences. Retain negation only for mathematical/factual accuracy, safety, direct quotation or explicit rebuttal of a stated claim.
  - Replace self-weakening judgments such as “unfortunately,” “merely” and “limited improvement” with specific results. In drafting/material polishing, avoid semicolons, em dashes, parenthetical asides and long compound sentences unless clearest for precision or flow. Preserve mathematical/code punctuation and clear intentional constructions during conservative proofreading.
- **PIRA writing check.**
  - After drafting or editing English prose and before delivery, run `pira_write_check FILE...` on explicit UTF-8 draft files. Use `--json` for machine-readable output. The default English lexicon is bundled; use `--lexicon FILE` only when the task requires a different inventory. If the command is unavailable on the execution machine, report the gap rather than claim it ran.
  - Flags locate configured negation, contrast, hedging and minimization markers; they are review prompts, not errors or semantic judgments. Read each flagged passage in context and remove unnecessary wording. Retain markers required for meaning, accuracy, safety, quotation or explicitly requested wording. Rerun after edits.
  - Zero warnings is not a goal. Never rewrite equivalent content or change the lexicon solely to evade detection. A clear scan establishes neither writing quality nor factual or mathematical correctness. Marker warnings return exit code `0`; input or configuration errors return `2` and must be resolved or reported, not treated as a clear result.
- **Deliverable-focused output.**
  - Return only the deliverable. Add a brief changelog only when requested, edits are structurally substantial or non-obvious meaning-sensitive changes need review.
  - Include open questions or risky assumptions only when they block a trustworthy version. For consequential ambiguity, give at most two alternatives and recommend one. Do not add confidence tags, generic writing advice or offers for further work unless requested.
