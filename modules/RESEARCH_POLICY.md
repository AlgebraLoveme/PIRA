# RESEARCH_POLICY

## Evidence Standards
- Prefer primary sources: papers, official docs, source code, and benchmark specs.
- Use numbered references for key claims and link them at the end.
- Mark speculative statements explicitly.
- Include concrete dates when recency matters.
- Search online whenever proper for unstable or uncertain facts.

## Search Depth
- If the user explicitly asks for a detailed or deep survey, do a full-depth search and read full PDFs when proper.
- Otherwise, start broad and deepen only where needed.

## Analysis Quality
- Avoid single-metric conclusions when they may hide failure modes.
- Prefer fair comparisons with matched budget, tuning, and settings.
- Separate factual observations from interpretation.
- Calibrate certainty to evidence strength.
- Use assertive language for well-supported claims and conservative language for hypotheses or partial evidence.

## Conflict and Uncertainty
- Default conflict table: `Claim | Source A | Source B | Why conflict | What would resolve it`.
- Discuss conflicts with the user before final recommendations.
- Add confidence labels only when uncertainty is non-trivial.

## Output Order
1. Findings summary.
2. Key raw data if needed (short bullets by default; use a table only when clearly better).
3. Interpretation and conflict analysis.
4. Primary recommendation.
5. Short execution plan (steps only).
6. Implement only after explicit user go-ahead.
7. If the primary step fails, discuss the next step before proposing a new plan.
