# PAPER_READING

## Requests
- **Triage a paper for relevance**
  - **Done when:** the user can decide whether to read, cite, or use the paper based on its problem, main claim, strongest visible support, consequential limitation, and fit to the stated goal.
  - **Workflow:** target decision → title, abstract, introduction, conclusion, and key figures or tables → claim, support, and risk → relevance verdict.
- **Understand a contribution or method**
  - **Done when:** the user can reconstruct the problem, operative idea, claimed capability, supporting evidence, and important boundary without reproducing the paper's exposition.
  - **Workflow:** question → problem and contribution → operative mechanism or inference chain → decisive evidence → assumptions and boundary. Read method, proof, or appendix detail only where the mechanism remains incomplete.
- **Evaluate evidence or critique a paper**
  - **Done when:** each material claim is tied to the evidence that does or does not support it, and the resulting judgment accounts for assumptions, uncertainty, comparison fairness, and plausible alternative explanations.
  - **Workflow:** claim inventory → evidence locations → design, assumptions, baselines, and uncertainty → alternative explanations → calibrated trust judgment.
- **Verify a claim or citation**
  - **Done when:** the target statement has a clear support verdict grounded in the cited paper's exact result and scope.
  - **Workflow:** target statement → exact passage, result, or theorem → surrounding method and scope → supported, partially supported, or unsupported. Distinguish mere topic relevance from actual entailment.
- **Extract implementation or reproduction details**
  - **Done when:** the user has a dependency-ordered implementation path and can distinguish the reported procedure, required inference, optional recommendation, and consequential unresolved choices.
  - **Workflow:** target behavior → method, appendix, supplement, and artifact → reported inputs and preprocessing → model or procedure → parameters and training → evaluation → blocking or result-sensitive gaps → separately labeled inferences or recommendations.
  - Default to two parts: the reported procedure in dependency order, then unresolved decisions. Mark any inference needed to make the procedure executable at the point where it occurs; do not silently resolve it. Add engineering recommendations only when requested. Prioritize missing details that prevent execution or can materially change the result; omit ordinary unspecified defaults unless they are outcome-sensitive.

Use the applicable request workflow plus the shared rules below.

## Reading Method
- Start with the smallest sections that can answer the question. Default to title, abstract, introduction, conclusion, and task-relevant figures or tables; inspect methods, proofs, appendices, supplements, and artifacts only as needed.
- Use three depth levels: triage scan, targeted reading, and one deliberate full pass. Escalate only when the user asks, the paper is central to an important decision, abstract-level claims remain consequentially uncertain, or close critique, implementation, or reproduction requires broader context.
- Ask one clarification only when different reading goals would materially change what must be inspected. Otherwise infer the goal, state any consequential assumption, and proceed.
- Skip references by default. Follow only citations needed to resolve the task; broader literature synthesis requires an explicit request.
- Keep compact notes in your own words. If the contribution or decisive inference cannot be restated simply, inspect the missing evidence rather than padding the explanation.

## Evidence and Critique
- Treat figures, tables, theorem statements, proofs, and key experimental results as evidence, not decoration. Check whether they support the headline claim at its stated scope.
- For empirical work, inspect the study design, data, baselines, ablations or sensitivity analyses, uncertainty, evaluation metric, and comparison fairness when relevant. For theoretical work, inspect assumptions, theorem statements, proof dependencies, and guarantee scope.
- Separate what the paper directly shows, what the authors infer, and what you infer. Label uncertainty and tentative critique; poor exposition alone does not imply invalid science.
- Challenge framing, assumptions, baselines, and alternative explanations without manufacturing objections. Check whether the evidence would support a narrower claim when it does not support the headline version.
- When the paper, supplement, artifact, or revisions conflict, preserve source and version provenance and do not silently choose among them. State what evidence could resolve the conflict; matching a reported output alone does not establish which procedure produced it.
- Cite paper locations as precisely as available, using section and paragraph, figure or table, theorem or proposition, appendix, or artifact location. Follow `RESEARCH_POLICY.md` for external sourcing and citation format.

## Output
- Lead with the answer, relevance verdict, support verdict, or trust judgment requested. Use only the request-specific structure needed to make that conclusion auditable.
- Include problem, method, evidence, limitations, transferable insights, or next steps only when they affect the user's question. Do not emit a fixed paper-summary template.
- Present each material fact or source conflict once. For multiple sources, annotate the dependency-ordered procedure inline or use a compact comparison, rather than repeating both, unless each view answers a distinct user need.
- State reading-depth or access limitations only when they constrain the conclusion. Never imply that an uninspected method, appendix, supplement, artifact, or reference was checked.

## Guardrails
- Do not overread by default, overstate what the paper proves, or rely only on its abstract or conclusion when the decision depends on underlying evidence.
- Do not present author interpretation or tentative critique as demonstrated fact.
