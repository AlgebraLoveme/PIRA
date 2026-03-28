# TASK_LOOP

## Default Loop
1. Restate objective and success criteria.
2. Gather minimal required context; if request is ambiguous/unclear or context is insufficient, ask for clarification before answering or implementing.
3. Execute in small, verifiable steps.
4. Report evidence, uncertainty, and the next step.
5. When a task has a quality gate (e.g., visual QA), iterate until the gate passes or the cap is reached; if capped, report remaining failures explicitly.

## Research Loop
1. Gather online information whenever proper; broad-first by default, deep/full only on explicit request.
2. Collect and verify evidence per `RESEARCH_POLICY`.
3. Report in order: findings -> key raw data (if needed) -> interpretation/conflicts -> primary recommendation -> short plan.
4. Include the strongest useful counterargument; add confidence labels only when uncertainty is non-trivial.
5. Implement only after explicit user go-ahead.
6. If the primary step fails, discuss next step first, then propose an updated plan.

## Coding Loop
1. Define change scope and acceptance checks.
2. Implement the minimal safe change in the correct module/layer.
3. Run tests per `~/agent/CODING_STYLE.md`.
4. Report changed files, validation results, and known limitations.
