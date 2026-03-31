# CODING_STYLE

## Priorities
- Use this global coding style by default; do not switch to repository-local style unless explicitly instructed.
- Prioritize correctness, clarity, and maintainability.
- Optimize only when evidence supports it or the effort is minimal; if a major optimization seems strongly recommended, raise the details and confirm with the user.

## Types and Naming
- Use type hints whenever proper, especially on function/method signatures; add them elsewhere when they improve clarity.
- Keep names concise by default; expand only when needed to remove ambiguity.
- When proposing names, give one best name by default; give multiple options only if explicitly requested.

## Design and Scope
- Prefer small, single-purpose functions/modules.
- Keep data flow explicit; avoid hidden side effects.
- Keep orchestration code thin; put method-specific logic in dedicated modules.
- Split long functions only when readability or maintainability clearly suffers.
- Keep configuration centralized; avoid scattered hardcoded constants.
- Prefer minimal-diff edits; touch only what is necessary.
- Avoid broad refactors unless explicitly requested.

## Performance and Dependencies
- Refactor for performance only with profiling/measurement or clear workload evidence of a real bottleneck.
- Prefer readable vectorized/batched implementations when cleanly possible.
- Use lower-level or more complex optimization only when needed.
- For non-obvious optimization, add a short comment explaining why it is needed and its main complexity/tradeoff.
- Prefer the standard library and existing dependencies first.
- Add new dependencies only when the material benefit is clear.
- For large features likely open-sourced, survey online for high-quality implementations; if you find one, raise it and confirm with the user.

## Code Optimization
- Before substantial optimization, define both correctness tests and speed tests.
- Show the planned tests before implementing substantial optimization work.
- Stabilize benchmarks before drawing conclusions; prefer more timed steps/repeats over quick, noisy results.
- Separate avoidable cost from unavoidable cost.
- Distinguish microbenchmark behavior from real-workload behavior.
- If an optimization is not convincing, stop and report the evidence rather than forcing further changes.

## Input Contracts and Errors
- Add lightweight runtime checks/assertions when assumptions are strict (e.g., shape/range/dtype/device).
- Avoid excessive defensive checks, especially in one-off analysis code or code that predictably will not be reused across settings; add only checks that are actually necessary.
- Put each check at the narrowest layer that truly requires it; if a condition matters only inside a function, check it there rather than in parent/orchestration code.
- When the effort is small, generalize code to handle more cases than the document promised when practical; if you do, revise the document accordingly.
- Keep checks concise and fail with clear, actionable messages.
- Default to fail-fast behavior; avoid silent fallback behavior.

## Logging, Docs, and Comments
- Default to concise structured logs at key steps (config, major stage start/end, critical metrics).
- Avoid verbose per-iteration logs unless debugging is explicitly needed; if used, show them only as postfix text on a terminal progress bar.
- Public APIs should have concise docstrings.
- Internal/helper docstrings are needed only when logic is non-obvious.
- Comments should explain intent, assumptions, and tradeoffs, not obvious syntax.
- When handling Tensors and a default shape is possible, infer the shapes of non-obvious commands—especially for clear shape-handling subroutines—and add inline shape comments; run small tests if needed to confirm important shapes.

## Reproducibility
- Add random seeding by default via a centralized `seed_everything(seed)` utility.
- Do not enforce other reproducibility metadata unless explicitly requested.

## Testing
- If the user specifies tests, run exactly those tests.
- Otherwise, run minimal fast checks by default, mainly syntax/grammar/static sanity.
- Avoid test runs expected to exceed about 30 seconds unless explicitly requested.
