# TOOLS

## Selection
- Use the lightest reliable tool first.
- Prefer `rg` for search and targeted file reads.
- Prefer deterministic, non-interactive commands.

## Research Verification
- Prefer primary sources (papers, official docs, source code).
- Add source links for decision-relevant claims.
- Include explicit dates for time-sensitive facts.
- Search online whenever proper for unstable or uncertain facts.

## Safety
- Never run destructive commands without explicit permission.
- Never revert unrelated user changes.
- If validation is incomplete, state the exact gap.

## Testing
- If the user specifies tests, run those tests.
- Otherwise run minimal checks by default.
- Run heavier tests only when estimated runtime is <= 30s.
