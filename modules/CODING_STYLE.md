# CODING_STYLE

## Workflow
1. Define scope and the smallest useful acceptance check.
2. Apply the lean ladder before adding code.
3. Make the minimal safe change.
4. Run the smallest relevant checks; report gaps.

## Lean Ladder
Stop at the first sufficient rung:
1. Need not exist → skip speculative code; say so briefly.
2. Standard library solves it → use it.
3. Native platform feature covers it → use it.
4. Installed dependency solves it cleanly → use it.
5. One clear line is sufficient → keep one line.
6. Otherwise write the minimum working code.

## Scope and Structure
- Use this global style unless trusted repository-local instructions specify otherwise; explicit user instructions override both.
- Use correct, boring, readable solutions rather than clever/speculative ones.
- Avoid unrequested abstractions, boilerplate, future scaffolding, and configuration for constants.
- Choose deletion over addition when possible; the fewest-file, shortest working diff wins.
- Keep data flow explicit and side effects narrow.
- Keep each function at one abstraction level. Extract only to name a real idea, remove duplication, or expose a boundary.
- Avoid flag arguments that create distinct behaviors; split behavior or use an explicit mode only when simpler.
- Centralize true configuration; avoid scattered hardcoded constants.
- For a complex request with a simpler sufficient solution, implement it and briefly name omissions; ask only when defaulting is risky.

## Boundaries and Refactoring
- Refactor only to reduce current-change risk, remove duplication, clarify a boundary, or materially ease testing.
- Preserve behavior first; before non-trivial refactoring, identify the smallest check protecting moved behavior.
- When cheap, isolate stable core logic from volatile CLI, file I/O, network, database, UI, framework, and subprocess details.
- Dependencies point from volatile outer code to stable inner logic; core logic must not import infrastructure merely for convenience.
- Pass simple data across boundaries; do not leak framework, ORM, request, or process objects into core logic unless the project is intentionally glue code.
- Improve boundaries incrementally within touched code; no architecture-wide or drive-by refactors. Leave in-scope touched code slightly cleaner.

## Names and Types
- Use type hints when appropriate, especially on function/method signatures.
- Names reveal intent, domain meaning, units, and important distinctions. Use one word for one concept; avoid misleading near-synonyms.
- Keep names concise unless expansion removes ambiguity. When proposing names, give one best choice by default.

## Dependencies and Performance
- Add a dependency only for clear material benefit when owning a few lines would be worse.
- Between equally small standard-library/platform options, choose better edge-case correctness.
- Optimize only with profiling, measurement, or clear workload evidence; stop when evidence is unconvincing.
- Explain non-obvious optimization tradeoffs in a short comment.
- For large likely-open-source features, survey high-quality online implementations; raise promising options and confirm with the user.

## Contracts, Errors, Security, and Shortcuts
- Never remove trust-boundary input validation, data-loss-preventing error handling, security behavior, accessibility basics, or real-hardware calibration knobs for simplicity.
- Security is behavior: preserve in-scope authentication, authorization, permission/scope checks, secret handling, safe parsing/escaping, injection/XSS/CSRF/SSRF protections, resource limits, crypto/TLS defaults, and audit-relevant logs.
- Add runtime checks only where strict assumptions matter, e.g. shape, range, dtype, device, trust, or security boundaries.
- Checks must be narrow, fail-fast, and actionable; avoid silent fallback unless explicitly requested.
- Keep error paths visible without obscuring main flow. Swallow/translate errors only when it adds actionable context.
- Failure/exception bug fix → include the smallest practical failure-path check.
- Mark intentional simplifications with `PIRA:`. If a shortcut has a known ceiling (e.g. global lock, $O(n^2)$ scan, naive heuristic), name the ceiling and upgrade path.

## Observability and Comments
- Default to concise structured logs for configuration, major-stage start/end, and critical metrics; avoid per-iteration logs unless explicitly debugging.
- Public APIs need concise docstrings; internal/helper docstrings only for non-obvious logic.
- Prefer clearer names/structure to explanatory comments. Comments cover intent, invariants, assumptions, ceilings, and tradeoffs—not obvious syntax.
- For non-obvious tensor-shape handling, infer and note shapes inline; run small tests when needed to confirm important shapes.

## Reproducibility
- For new Python workflows with stochastic behavior and no project convention, add centralized `seed_everything(seed)` by default. In other languages or established projects, follow the local convention.
- Add no further reproducibility metadata unless explicitly requested.

## Checks and Tests
- Non-trivial new logic needs the smallest runnable check that fails if it breaks; trivial one-liners need no tests.
- Tests must be readable, independent, fast, and focused on observable behavior, not implementation shape.
- Run user-specified tests first. Otherwise default to minimal fast checks: syntax, grammar, static sanity, or focused smoke test.
