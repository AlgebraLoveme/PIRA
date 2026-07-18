# PAPER_READING

## Goal and Strategy
- Read one research paper efficiently; default to the minimum decision-useful extraction.
- First identify the goal: relevance/triage, main idea/background, method, evidence quality, limitations, citation support, implementation, critique, reproduction, or review. Goal determines depth; if unclear from context, confirm before reading.
- Escalate to one full read only when context clearly requires it or the user asks. Always end with a depth-scaled structured note.

## Context-Efficient Reading
- Do not read the whole paper by default. Start with title, abstract, introduction, figures/tables, and conclusion.
- Skip references by default. Read methods, appendices, proofs, supplement, or artifact details only when relevant to the goal.
- Inspect references only for reference checking or a clearly important citation; search that citation rather than reading the full list.

## Progressive Depth
1. **Triage:** problem, main claim, potential importance.
2. **Core understanding:** method, supporting evidence, important assumptions/limits.
3. **Full read:** one front-to-back pass, still skipping references, only when full understanding, close critique, or implementation/reproduction detail is needed.

Perform a full read when the user asks, the paper is project-central, an important decision rests on a strong abstract-level story, the paper appears contradictory/suspicious/unusually influential, or method detail is needed for implementation/critique. Keep it one deliberate pass, not uncontrolled detail chasing.

## Evidence and Claims
- Treat figures, tables, theorems, and key experimental results as primary evidence; verify that they support the headline claim.
- Empirical paper: inspect baselines, ablations, uncertainty, and comparison fairness.
- Theory paper: inspect assumptions, theorem statements, and guarantee scope.
- Separate what the paper directly shows, what authors infer, and what you infer; never merge them silently.
- Take short notes in your own words: core claim, method sketch, strongest evidence, main assumptions, key limitation/doubt. Inability to restate the contribution simply signals incomplete understanding.

## References and Critique
- Follow only task-useful references: one foundational precursor, one strongest baseline/comparator, and one important follow-up/response. Do not turn one-paper reading into an unbounded survey unless asked.
- Challenge assumptions, framing, baselines, and alternative explanations; check your own confirmation bias. Poor exposition does not imply invalid science.

## Structured Note and Citations
- Focus the note on claims, support, and uncertainty. If the task shifts to teaching, use `LEARNING_STYLE.md`; search online for background when needed.
- Cite paper locations when practical and precisely when available: section + paragraph, figure/table, theorem/lemma/proposition, or appendix section (for example, `Section 2, second paragraph`, `Figure 3`, `Appendix B, first paragraph`).
- Reuse one numbered reference for repeated citations to the same source; list its link once at the end, e.g. `(Table 1, [1])`, `(Table 2, [1])`, then `[1] <link>`.

## Default Output
1. Problem.
2. Main claim/contribution.
3. Method.
4. Evidence.
5. Assumptions/limitations.
6. Reliable transferable takeaways when feasible: reusable method insights, engineering tricks, design choices, or evaluation practices.
7. Overall trust/usefulness.
8. Next step, if any.

Include relevance to the user's goal only when clear and decision-useful. Transferable takeaways are optional by default but strongly encouraged when reliably inferable from read sections.

## Full-Read Output
1. One-paragraph summary.
2. Problem setting.
3. Key idea.
4. Method.
5. Evidence.
6. Strengths.
7. Weaknesses.
8. Assumptions.
9. Transferable takeaways.
10. Open questions.
11. User relevance, only when clear/useful.

Transferable takeaways are mandatory after a full read; if none are meaningful, say so explicitly.

## Guardrails
- Do not overread by default, overstate what the paper proves, or rely only on abstract/conclusion when the decision matters.
- Do not present tentative critique as fact or expose reading-process metadata unless directly useful.
- Keep teaching policy in `LEARNING_STYLE.md`; do not turn this module into general teaching.
- Use precise locations when available; do not repeat source links when one grouped numbered reference suffices.
