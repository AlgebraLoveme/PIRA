# PAPER_READING

## Goal
- Read a single research paper efficiently.
- Default to extracting only the most decision-useful information to save context.
- When the task or context clearly requires it, read the full paper once and return a structured note.

## Default Strategy
1. Identify the user's goal: triage, background understanding, citation check, implementation, critique, reproduction, or review.
2. Read only the minimum parts needed for that goal by default.
3. Escalate to one full read only when the need is clear from context or the user asks.
4. End with a structured note.

## Rule 1: Start with the question
- First identify what the user needs from the paper.
- Common goals:
  - relevance check
  - main idea
  - method understanding
  - evidence quality
  - limitations
  - implementation details
  - citation support
- Let the goal determine reading depth.
- If the goal is unclear from context, confirm before reading.

## Rule 2: Default to context-efficient reading
- Do not read the whole paper by default.
- Start with the highest-yield parts:
  - title
  - abstract
  - introduction
  - figures and tables
  - conclusion
- Skip the reference section by default; it rarely helps single-paper understanding directly.
- Read methods, appendices, proofs, supplement, or artifact details only when they matter for the goal.
- Check references only when the user wants reference checking or one specific citation is clearly important; search for the specific reference instead of reading the full reference section.
- If a paper or source bundle must be downloaded temporarily, store it in the platform's default temp directory unless the user wants it kept:
  - macOS: `$TMPDIR`
  - Linux: `/tmp`
  - Windows: `%TEMP%` or `%TMP%`

## Rule 3: Use progressive depth
- Depth 1: quick triage
  - What problem is addressed?
  - What is the main claim?
  - Why might it matter?
- Depth 2: core understanding
  - What is the method?
  - What evidence supports it?
  - What assumptions or limits matter?
- Depth 3: full read
  - Read the paper once front to back, still skipping references, when the task clearly needs full understanding, close critique, or implementation/reproduction detail.

## Rule 4: Prefer evidence over narration
- Treat figures, tables, theorems, and key experimental results as primary evidence.
- Check whether the evidence actually supports the headline claim.
- For empirical papers, inspect baselines, ablations, uncertainty, and fairness of comparison.
- For theory papers, inspect assumptions, theorem statements, and scope of guarantees.

## Rule 5: Separate layers of claim
- Distinguish:
  - what the paper directly shows
  - what the authors infer
  - what you infer
- Do not silently merge these.

## Rule 6: Read actively
- Write short notes in your own words.
- Record:
  - core claim
  - method sketch
  - strongest evidence
  - main assumptions
  - key limitation or doubt
- If you cannot restate the contribution simply, you likely do not understand it yet.

## Rule 7: Escalate to a full read when clearly warranted
- Read the full paper once when:
  - the user explicitly asks
  - the paper is central to the user's project
  - the abstract-level story seems strong but the decision is important
  - the paper looks contradictory, suspicious, or unusually influential
  - method details are needed for implementation or critique
- A full read should still be a single deliberate pass, not uncontrolled detail chasing.

## Rule 8: Follow references selectively
- Do not read the reference section by default.
- Only inspect references that are directly useful for the task:
  - one foundational precursor
  - one strongest baseline or comparator
  - one important follow-up or response
- Do not turn single-paper reading into an unbounded survey unless asked.

## Rule 9: Be critical but fair
- Challenge assumptions, framing, baselines, and alternative explanations.
- Watch for your own confirmation bias too.
- Do not confuse poor exposition with invalid science.

## Rule 10: End with a structured note
- Always return a structured note, scaled to the reading depth.
- Keep the note focused on what the paper says, what supports it, and what remains uncertain.
- If the main need shifts from reading to teaching the material, use `LEARNING_STYLE.md` for explanation style and search online for background material when needed.
- When referencing paper content in the output, cite its location when practical.
- Prefer precise locations: section and paragraph, figure/table number, theorem/lemma/proposition number, appendix section.
- Example style: `Section 2, second paragraph`, `Figure 3`, or `Appendix B, first paragraph`.
- If multiple citations use the same source link, reuse one numbered reference and place the link once at the end.
- Example style: `(Table 1, [1])` and `(Table 2, [1])`, with `[1] <link>` listed once in the references.

## Default Output
1. Problem the paper addresses.
2. Main claim or contribution.
3. Method summary.
4. Evidence summary.
5. Assumptions and limitations.
6. Transferable takeaways, when feasible to infer from the sections read:
   - insights that may help future methods
   - engineering tricks, design choices, or evaluation practices worth reusing
7. Overall take:
   - trust
   - usefulness
8. Next step, if any.
- Include relevance to the user's goal only when it is clear and decision-useful.
- In default mode, transferable takeaways are optional but strongly encouraged when they can be inferred reliably from the read sections.

## Full-Read Output
- When a full read is performed, structure the note as:
  1. one-paragraph summary
  2. problem setting
  3. key idea
  4. method
  5. evidence
  6. strengths
  7. weaknesses
  8. assumptions
  9. transferable takeaways
  10. open questions
  11. relevance to the user, only when clear and useful
- In full-read mode, transferable takeaways are mandatory.
- If no meaningful transferable takeaway is found, state that explicitly rather than omitting the item.

## Guardrails
- Do not overread by default.
- Do not overstate what the paper proves.
- Do not rely only on abstract and conclusion when the decision matters.
- Do not present tentative critique as fact.
- Do not report reading-process metadata to the user unless it is directly useful for the task.
- Do not turn this module into a general teaching module; keep explanation policy in `LEARNING_STYLE.md`.
- Do not use vague citations when a more precise location is available.
- Do not repeat the same source link inline when one grouped numbered reference will do.
- Do not leave temporary paper downloads or extracted sources in the workspace unless the user wants them preserved.
