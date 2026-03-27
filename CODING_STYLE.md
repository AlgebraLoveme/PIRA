# CODING_STYLE

Applies only when coding is explicitly requested.

## Conventions and Priorities
- Prioritize this global coding style by default; do not switch to repository-local style unless explicitly instructed.
- Prioritize correctness, clarity, and maintainability.
- Optimize only when there is evidence that it is needed.

## Type Hints and Naming
- Use type hints whenever proper, especially for function/method signatures; add them elsewhere when they improve clarity.
- Keep names short by default; expand when ambiguity appears.
- When proposing names, provide one best name by default; provide multiple options only if explicitly requested.

## Design and Change Scope
- Use small, single-purpose functions/modules.
- Keep data flow explicit; avoid hidden side effects.
- Keep orchestration code thin; put method-specific logic in dedicated modules.
- Split long functions only when readability/maintainability clearly suffers.
- Keep configuration centralized; avoid scattered hardcoded constants.
- Prefer minimal-diff edits; touch only what is necessary.
- Avoid broad refactors unless explicitly requested.

## Performance and Dependencies
- Refactor for performance only when profiling/measurement or clear workload evidence shows a real bottleneck.
- Default to readable vectorized/batched implementations when cleanly possible.
- Use lower-level/complex optimization only when needed.
- For non-obvious optimization, add a short comment on why it is needed and its main complexity/tradeoff.
- Prefer standard library and existing dependencies first.
- Add new dependencies only with clear material benefit.

## Input Contracts and Errors
- Add lightweight runtime checks/assertions by default when assumptions are strict (e.g., shape/range/dtype/device).
- Keep checks concise and fail with clear actionable messages.
- Default to fail-fast behavior; avoid silent fallback behavior.

## Logging, Documentation, and Comments
- Default to concise structured logs at key steps (config, major stage start/end, critical metrics).
- Avoid verbose per-iteration logs unless debugging is explicitly needed.
- Public APIs should have concise docstrings.
- Internal/helper docstrings only when logic is non-obvious.
- Comments should explain intent, assumptions, and tradeoffs (not obvious syntax).

## Reproducibility
- Add random seed by default.
- Prefer a centralized `seed_everything(seed)` utility.
- Do not enforce other reproducibility metadata unless explicitly requested.

## Testing
- If the user specifies tests, run exactly those tests.
- Otherwise run minimal, fast checks by default (mainly syntax/grammar/static sanity).
- Avoid test runs expected to exceed ~30 seconds unless explicitly requested.
- If a touched module has no tests, add one minimal regression test by default when it fits the runtime cap.
