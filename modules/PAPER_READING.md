# PAPER_READING

## Requests
- **Triage for relevance**
  - **Done when:** the user can decide whether to read, cite, or use the paper from its problem, main claim, strongest visible support, consequential limitation, and fit to the stated goal.
  - **Workflow:** target decision → title, abstract, introduction, conclusion, and key figures or tables → claim, support, and risk → relevance verdict.
- **Understand a contribution or method**
  - **Done when:** the user can reconstruct the problem, operative idea, claimed capability, supporting evidence, and important boundary without reproducing the exposition.
  - **Workflow:** question → problem and contribution → mechanism or inference chain → decisive evidence → assumptions and boundary. Inspect method, proof, or appendix detail only where the mechanism remains incomplete.
- **Evaluate evidence or critique**
  - **Done when:** each material claim is tied to evidence that does or does not support it, and the judgment accounts for assumptions, uncertainty, comparison fairness, and plausible alternative explanations.
  - **Workflow:** claim inventory → evidence locations → design, assumptions, baselines, and uncertainty → alternatives → calibrated trust judgment.
- **Verify a claim or citation**
  - **Done when:** the target statement has a clear support verdict grounded in the cited paper's exact result and scope.
  - **Workflow:** statement → exact passage, result, or theorem → surrounding method and scope → supported, partially supported, or unsupported. Topic relevance alone is not entailment.
- **Extract implementation or reproduction details**
  - **Done when:** the user has a dependency-ordered implementation path and can distinguish reported procedure, required inference, optional recommendation, and consequential unresolved choices.
  - **Workflow:** target behavior → method, appendix, supplement, and artifact → reported inputs and preprocessing → model or procedure → parameters and training → evaluation → blocking or result-sensitive gaps → separately labeled inferences or recommendations.
  - Default to two parts: the reported procedure in dependency order, then unresolved decisions. Mark any inference needed to make the procedure executable where it occurs; do not resolve it silently. Add engineering recommendations only when requested. Prioritize gaps that block execution or materially affect results; omit ordinary unspecified defaults unless outcome-sensitive.

Use the applicable workflow plus the shared rules below.

## Reading Method
- Start with the smallest sections that can answer the question. Default to title, abstract, introduction, conclusion, and relevant figures or tables; inspect methods, proofs, appendices, supplements, and artifacts only as needed.
- Use three depths: triage scan, targeted reading, and one deliberate full pass. Escalate only on request, for a paper central to an important decision, when abstract-level claims remain consequentially uncertain, or when close critique, implementation, or reproduction needs broader context.
- Ask one clarification only when the reading goal materially changes what must be inspected; otherwise infer the goal, state consequential assumptions, and proceed.
- Skip references by default. Follow only citations needed for the task; broader literature synthesis requires an explicit request.
- Keep compact notes in your own words. If the contribution or decisive inference cannot be stated simply, inspect missing evidence rather than pad the explanation.

## Evidence and Critique
- Treat figures, tables, theorem statements, proofs, and key experimental results as evidence, not decoration; test whether they support the headline claim at its stated scope.
- For empirical work, inspect relevant design, data, baselines, ablations or sensitivity analyses, uncertainty, metrics, and comparison fairness. For theoretical work, inspect assumptions, theorem statements, proof dependencies, and guarantee scope.
- Separate what the paper directly shows, what its authors infer, and what you infer. Label uncertainty and tentative critique; poor exposition alone does not imply invalid science.
- Challenge framing, assumptions, baselines, and alternative explanations without manufacturing objections. If evidence misses the headline scope, test whether it supports a narrower claim.
- Preserve source and version provenance when paper, supplement, artifact, or revisions conflict; do not choose silently. State what could resolve the conflict. Matching a reported output alone does not identify the producing procedure.
- Cite the most precise available location: section and paragraph, figure or table, theorem or proposition, appendix, or artifact. Follow `RESEARCH_POLICY.md` for external sources and citation format.

## Output
- Lead with the requested answer, relevance or support verdict, or trust judgment. Use only enough structure to make it auditable.
- Include problem, method, evidence, limitations, transferable insights, or next steps only when they affect the question; do not emit a fixed paper-summary template.
- State each material fact or conflict once. For multiple sources, annotate a dependency-ordered procedure inline or use a compact comparison; repeat views only when they serve distinct needs.
- State depth or access limits only when they constrain the conclusion. Never imply that uninspected methods, appendices, supplements, artifacts, or references were checked.

## Guardrails
- Do not overread by default, overstate what the paper proves, or rely only on abstract or conclusion when the decision needs underlying evidence.
- Do not present author interpretation or tentative critique as demonstrated fact.
