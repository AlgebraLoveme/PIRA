# CODING_STYLE

## Conventions and Priorities
- Prioritize this global coding style by default; do not switch to repository-local style unless explicitly instructed.
- Prioritize correctness, clarity, and maintainability.
- Optimize only when there is evidence that it is needed, or when the effort is minimal; if you believe a major optimization is strongly recommended, raise it with details and confirm with the user.

## Type Hints and Naming
- Use type hints whenever proper, especially for function/method signatures; add them elsewhere when they improve clarity.
- Keep names concise by default; expand when ambiguity appears.
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
- For large features probably open-sourced, survey online for high-quality implementations; if available, raise it and confirm with the user.

## Input Contracts and Errors
- Add lightweight runtime checks/assertions by default when assumptions are strict (e.g., shape/range/dtype/device).
- When the effort is small, target the generalization of your code to handle as many cases as possible even when the document promised only sub-case; when this is true, revise the document accordingly as well.
- Keep checks concise and fail with clear actionable messages.
- Default to fail-fast behavior; avoid silent fallback behavior.

## Logging, Documentation, and Comments
- Default to concise structured logs at key steps (config, major stage start/end, critical metrics).
- Avoid verbose per-iteration logs unless debugging is explicitly needed; such logs should only be visualized as the postfix of a progress bar in the terminal.
- Public APIs should have concise docstrings.
- Internal/helper docstrings only when logic is non-obvious.
- Comments should explain intent, assumptions, and tradeoffs (not obvious syntax).
- When handling Tensors and a default shape is possible, infer the shape of non-obvious commands, especially when shape-handling is a clear subroutine; put the shape as inline comments. If necessary, run small tests to confirm important shapes.

## Reproducibility
- Add random seed by default via a centralized `seed_everything(seed)` utility.
- Do not enforce other reproducibility metadata unless explicitly requested.

## Testing
- If the user specifies tests, run exactly those tests.
- Otherwise run minimal, fast checks by default (mainly syntax/grammar/static sanity).
- Avoid test runs expected to exceed ~30 seconds unless explicitly requested.
