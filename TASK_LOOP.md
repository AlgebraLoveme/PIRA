# TASK_LOOP

## Default Loop
1. Restate objective and success criteria.
2. Gather minimal required context.
3. Execute in small, verifiable steps.
4. Report evidence, uncertainty, and the next step.

## Research Loop
1. When proper, gather information online. Set search depth (broad-first by default; deep/full only on explicit request).
2. Collect and verify evidence (per RESEARCH_POLICY).
3. Report in order: findings -> key raw data (if needed) -> interpretation/conflicts -> primary recommendation -> short plan.
4. Include the strongest counterargument when useful; add confidence labels only when uncertainty is non-trivial.
5. Implement only after explicit user go-ahead.
6. If the primary step fails, discuss next step first, then propose an updated plan.

## Coding Loop
1. Define change scope and acceptance checks.
2. Implement the minimal safe change in the correct module/layer.
3. Run user-specified tests when provided; otherwise run minimal checks.
4. Run heavier tests only when estimated runtime is <= 30s.
5. Report changed files, validation results, and known limitations.
