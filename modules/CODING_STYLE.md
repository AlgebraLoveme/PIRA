# CODING_STYLE

## Coding Loop
1. Define change scope and acceptance checks.
2. Implement the minimal safe change in the correct module or layer.
3. Run tests.
4. Report changed files, validation results, and known limitations.

## Priorities
- Use this global coding style by default; switch to repository-local style only on explicit instruction.
- Prioritize correctness, clarity, and maintainability.
- Optimize only when evidence supports it or the effort is minimal; if a major optimization seems strongly recommended, raise the case and confirm with the user.

## Types and Naming
- Use type hints whenever proper, especially on function or method signatures, and elsewhere when they materially improve clarity.
- Keep names concise unless expansion removes ambiguity.
- When proposing names, give one best choice by default; give multiple options only if explicitly requested.

## Design and Scope
- Prefer small single-purpose functions or modules with explicit data flow and minimal hidden side effects.
- Keep orchestration thin and put method-specific logic in dedicated modules.
- Split long functions only when readability or maintainability clearly suffers.
- Centralize configuration; avoid scattered hardcoded constants.
- Make minimal-diff edits and avoid broad refactors unless explicitly requested.

## Performance and Dependencies
- Refactor for performance only with profiling, measurement, or clear workload evidence of a real bottleneck.
- Prefer readable vectorized or batched implementations when cleanly possible; use lower-level or more complex optimization only when needed.
- For non-obvious optimization, add a short comment explaining why it is needed and its main complexity or tradeoff.
- Prefer the standard library and existing dependencies first; add new dependencies only when the material benefit is clear.
- For large features likely to be open-sourced, survey online for high-quality implementations, then raise and confirm any promising one with the user.

## Optimization Workflow
- Before substantial optimization, define correctness tests and speed tests, and show the planned tests before implementing.
- Stabilize benchmarks before drawing conclusions; prefer more timed steps or repeats over quick noisy results.
- Separate avoidable from unavoidable cost and microbenchmark behavior from real-workload behavior.
- If an optimization is not convincing, stop and report the evidence rather than forcing more changes.

## Input Contracts and Errors
- Add lightweight runtime checks or assertions only where strict assumptions truly matter (for example shape, range, dtype, or device).
- Avoid excessive defensive checks, especially in one-off analysis code or code unlikely to be reused broadly.
- Put each check at the narrowest layer that requires it.
- When the effort is small, generalize code to handle more cases than promised when practical, and revise the document accordingly.
- Keep checks concise, fail fast, and use clear actionable messages; do not add silent fallbacks unless explicitly requested.

## Logging, Docs, and Comments
- Default to concise structured logs at key steps such as config, major stage start/end, and critical metrics.
- Avoid verbose per-iteration logs unless debugging is explicitly needed; if used, show them only as postfix text on a terminal progress bar.
- Public APIs should have concise docstrings; internal/helper docstrings are needed only when logic is non-obvious.
- Comments should explain intent, assumptions, and tradeoffs, not obvious syntax.
- For non-obvious tensor-shape handling, infer and note shapes inline; run small tests if needed to confirm important shapes.

## Reproducibility
- Add random seeding by default via a centralized `seed_everything(seed)` utility.
- Do not enforce additional reproducibility metadata unless explicitly requested.

## Testing
- If the user specifies tests, run exactly those tests.
- Otherwise run minimal fast checks by default, mainly syntax, grammar, or static sanity.
- Avoid test runs expected to exceed about 30 seconds unless explicitly requested.
