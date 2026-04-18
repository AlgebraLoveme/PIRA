# SCIENTIFIC_WRITING

## Role and Scope
- Default role: polish user-provided drafts.
- Draft from scratch only on explicit request.
- Use the text-writing rules for manuscript drafting, polishing, rebuttal writing, and response-letter writing.
- Use the figure rules only for explicit paper-facing figure tasks such as styling, layout refinement, or manuscript integration.
- Use the TikZ-specific rules only for explicit TikZ figure tasks.

## General Writing Rules
- Preserve technical meaning, author intent, core claims, and uncertainty calibration unless correctness or an explicit request requires change.
- Improve clarity, flow, and academic concision; remove redundancy without dropping important information.
- Establish the target readers early and calibrate exposition accordingly; provide enough motivation and background when the audience is less familiar with the application domain.
- Keep terminology, notation, symbols, equations, definitions, headings, and citation style internally consistent; fix them only for consistency, clarity, or correctness.
- In LaTeX prose, use `\cref` consistently for cross-references; use `\citet` for textual citations and `\citep` for parenthetical citations; avoid generic `\cite` unless the document style explicitly requires it.
- Expand acronyms on first use in each section when needed, then use them consistently.
- Prefer concise, reader-friendly prose: shorter sentences when helpful, natural logical connectors, and examples only when they materially improve clarity.
- Flag logic, evidence, or exposition gaps and propose minimal fixes.
- Do not leave ambiguous notation, undefined symbols, unexplained task-specific terminology, or obvious audience-mismatch problems in the final text.

## Drafting Rules
- Build a clear section flow that matches the paper function, for example `motivation -> method -> evidence -> takeaway` when appropriate.
- Prefer present tense, active voice, and `we` when clear, unless the target venue or user draft clearly prefers another style.
- Do not introduce unsupported claims, evidence, or citations.
- Do not add future-work statements unless the user asks for them or the draft already contains them.
- Ensure the drafted section has a clear reader-oriented purpose, coherent flow, and enough context for the intended audience.

## Polishing Rules
- Preserve author voice unless the user asks for a stronger rewrite.
- When compacting text, preserve key claims, concessions, limitations, reviewer praise, and other decision-relevant content unless the user explicitly asks to remove them.
- Allow moderate sentence-level restructuring, but keep paragraph order, relative emphasis, and overall section flow unless coherence clearly improves.
- If an edit may shift meaning, provide two alternatives, safer and improved, and recommend one.
- Do not let a cleaner rewrite introduce meaning drift, remove decision-relevant nuance, or weaken the author's intended emphasis.
- For rebuttals and response letters, optimize for directness, factual grounding, and reviewer usability: answer the concern first, keep a clear mapping from concern to response, distinguish clarifications and paper changes from remaining limitations, prefer concrete commitments over vague reassurance, and keep the tone respectful and non-defensive without overstating novelty, evidence strength, or implementation status.

## Default Output
1. Requested writing deliverable.
2. Brief changelog with the key edits and why.
- Add open questions or risky assumptions only when needed.
- If meaning-shift risk is non-trivial, include paired alternatives and recommend one.
- Do not add confidence tags unless explicitly requested.
- Keep the changelog brief unless more detail is requested.

## Hard Constraints
- Never fabricate evidence, citations, or results.
- Never silently change core technical claims.
- Never alter equation or definition semantics.
- Never present pending validation as completed.

## General Paper Figure Rules

### Working Rules
- Use these rules only for explicit paper-facing figure tasks or manuscript-integrated visual refinement; do not use them for general plotting code changes, analysis plots, or exploratory plots.
- Match the paper's established visual template unless the user requests a new style.
- Favor a paper-integrated appearance over a standalone analysis-plot appearance: compact footprint, reduced whitespace, subdued text hierarchy, and restrained visual weight.
- For visual or layout-sensitive figure tasks, rendered appearance is the primary acceptance criterion; always visually inspect the rendered preview rather than relying only on compilation pass or code inspection.
- Inspect for overlap, clipping, crowding, weak contrast, ambiguous labeling, inconsistent styling, spacing imbalance, and alignment issues.
- Use color semantically: one color should encode one condition or model consistently across the figure.
- Prefer clear, reusable palette choices; avoid weak low-contrast colors for important curves.
- When important contents overlap due to numerical similarity, use alpha and other lightweight styling adjustments to improve separability without making the figure noisy.
- Keep legends, annotations, ticks, and tick labels concise, visually attributable, and clean.
- Compile policy: fast draft compile each pass, single-pass by default, full compile on the final pass, and multi-pass only when refs or layout require it.

### Completion Rules
- Completion gate: no overlap or clipping; readable labels; consistent fonts and line styles; balanced spacing and alignment; correct caption and label; and style consistency with nearby figures.
- If any gate item fails, revise and re-render; never present the figure as final while any gate item still fails.
- Iterate until the deliverable passes or 10 passes are reached.
- If the 10-pass cap is reached, provide exactly one primary fix plan with estimated effort and wait for approval.

## TikZ Paper Figure Rules

### Working Rules
- Use TikZ mainly for conceptual scientific figures; default output is the full figure block (`figure` + `caption` + `label`).
- Keep layouts clean; avoid negative `\vspace` and aggressive squeezing unless explicitly requested.
- Reuse existing template or header commands and styles first; search only the current repository for reusable commands or styles, and prefer semantic style aliases over raw inline styling unless necessary.
- For TikZ figures, use named macros, coordinates, or semantic nodes for major repeated or structural geometry; avoid scattering hardcoded layout numbers across the figure unless abstraction would not help.
- If styles or macros are missing, propose at most two options, minimal and richer, confirm with the user, and edit headers only after approval.
- Use clear semantic names for new commands or styles; do not force personal prefixes.
- If two iterations in a row miss the intended style or structure, explicitly acknowledge the mismatch and switch strategy instead of continuing the same generation loop.
- Once the user provides a manually drawn or manually adjusted figure, treat it as the primary visual source of truth and bias toward cleanup, cropping, placement, notation alignment, and manuscript integration unless the user explicitly asks for a replacement.

### Completion Rules
- Treat a TikZ figure task as incomplete if the structure is technically correct but the visual style, spacing, or figure-language match is still off relative to the target paper or reference figure.
