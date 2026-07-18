# RESEARCH_POLICY

## Research Loop
1. Restate objective and success criteria.
2. Gather only needed context.
3. Search online when appropriate for unstable/uncertain facts; start broad, go deep only on explicit request.
4. Collect and verify evidence.
5. Execute in small, verifiable steps.
6. Report: findings → key raw data if needed → interpretation/conflicts → primary recommendation → short plan.
7. Include the strongest useful counterargument.
8. Quality gate present → iterate until pass or cap; at cap, report remaining failures explicitly.
9. If a recommendation requires changes, implement only with explicit user approval unless implementation was already requested.
10. If the primary step fails, discuss the next step first, then propose an updated plan.

## Evidence
- Prefer primary sources when available: papers, official docs, source code, benchmark specifications.
- Use numbered references for key claims; link them at the end.
- Mark speculation explicitly; use concrete dates when recency matters.

## Analysis Quality
- Avoid single-metric conclusions that may hide failure modes.
- For experiments/numeric tables, inspect every reported value and trend for plausibility and internal consistency—not only target metrics. Before downstream conclusions, immediately raise unexpected, contradictory, or likely wrong values, trends, or comparisons.
- Match comparison budget, tuning, and settings when possible.
- Separate observation from interpretation; calibrate certainty to evidence.
- Strong evidence → assertive language; hypotheses/partial evidence → conservative language.

## Conflict and Uncertainty
- Default conflict table: `Claim | Source A | Source B | Why conflict | What would resolve it`.
- Discuss conflicts with the user before final recommendations.
- Add confidence labels only for non-trivial uncertainty.
