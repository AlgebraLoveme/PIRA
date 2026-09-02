# CODING_STYLE

## Requests
- **Implement X:** deliver the smallest complete implementation with sufficient scope-specific checks.
- **Optimize X:** start from existing code; use behavior-preserving refactors for readability and in-scope extensibility; improve reliability, security, resource efficiency, or measured speed.

Executed checks bound claims that code works; report exact gaps. Other coding tasks use relevant rules.

## Workflow
1. Define scope and the smallest useful acceptance check.
2. Choose the first sufficient rung: omit unnecessary code and briefly say so → standard library → native platform feature → cleanly suitable installed dependency → one clear line → minimum working code.
3. Make the minimal safe change: correct, boring, readable over clever or speculative; prefer deletion when possible and the fewest-file, shortest working diff.
4. Run the smallest relevant checks; report gaps.

Before non-trivial refactoring, protect moved behavior with the smallest check. Refactor only to improve in-scope readability or required extensibility, reduce current-change risk or duplication, clarify boundaries, or materially ease testing. **Optimize** must prefer proportionate, behavior-preserving readability and extensibility refactoring; behavior changes require explicit user authorization. Do not add complexity for marginal gains unless the user explicitly requests it. Optimize performance only with profiling, measurement, or clear workload evidence; stop when evidence is unconvincing. Briefly note non-obvious tradeoffs.

## Change Discipline
- Use this global style unless trusted repository-local instructions specify otherwise; explicit user instructions override both.
- Avoid unrequested abstractions, boilerplate, future scaffolding, and configuration for constants.
- Keep data flow explicit and side effects narrow, with one abstraction level per function. Extract only to name a real idea, remove duplication, or expose a boundary.
- Avoid flag arguments that create distinct behaviors; split behavior or use an explicit mode only when simpler. Centralize true configuration and avoid scattered hardcoded constants.
- For a complex request, implement a simpler sufficient solution and briefly name omissions; ask only when defaulting is risky.
- When cheap, isolate stable core logic from volatile infrastructure such as CLI, I/O, network, database, UI, frameworks, and subprocesses; dependencies point inward. Pass simple data across boundaries. Core logic must not import infrastructure merely for convenience. Do not leak infrastructure objects into core logic unless the project is intentionally glue code.
- Improve boundaries incrementally in touched code. Do not make architecture-wide or drive-by refactors; leave in-scope touched code slightly cleaner.

## Names, Types, and Dependencies
- Use type hints when appropriate, especially on function/method signatures.
- Names reveal intent, domain meaning, units, and important distinctions. Use one word per concept; avoid misleading near-synonyms. Stay concise unless expansion removes ambiguity, and propose one best name by default.
- Add a dependency only for clear material benefit when owning a few lines would be worse. Between equally small standard-library/platform options, choose better edge-case correctness.
- For large, likely open-source features, survey high-quality online implementations; raise promising options and confirm with the user.

## Contracts, Failures, and Security
- Never simplify away trust-boundary validation, data-loss-preventing error handling, security behavior, accessibility basics, or real-hardware calibration controls.
- Preserve in-scope authentication, authorization, permission and scope checks, secret handling, safe parsing and escaping, injection/XSS/CSRF/SSRF protections, resource limits, crypto/TLS defaults, and audit-relevant logs.
- Add runtime checks only where strict assumptions matter, such as shape, range, dtype, device, trust, or security boundaries. Checks must be narrow, fail fast, and actionable; avoid silent fallback unless explicitly requested.
- Keep error paths visible without obscuring the main flow. Swallow/translate errors only to add actionable context. Failure/exception bug fixes require the smallest practical failure-path check.
- Mark intentional simplifications with `PIRA:`. For a known shortcut ceiling, name the ceiling and upgrade path.

## Operability
- Default to concise structured logs for configuration, major-stage start/end, and critical metrics; avoid per-iteration logs except during explicit debugging.
- Public APIs need concise docstrings; internal/helper docstrings only for non-obvious logic. Prefer clear names/structure to comments; comments explain intent, invariants, assumptions, ceilings, and tradeoffs, not obvious syntax.
- For non-obvious tensor-shape handling, infer and note shapes inline; run small tests when needed to confirm important shapes.
- For new stochastic Python workflows without a project convention, add centralized `seed_everything(seed)` by default. Otherwise follow the language or project convention. Add no further reproducibility metadata unless requested.

## Verification
- Non-trivial new logic needs the smallest runnable check that fails if it breaks; trivial one-liners need no tests.
- Tests must be readable, independent, fast, and focused on observable behavior rather than implementation shape.
- For medium- or high-level behavior, include the smallest diverse set of boundary tests that provides most of the assurance. Prefer cases that exercise multiple boundaries at once.
- Run user-specified tests first; otherwise use minimal fast checks such as syntax, grammar, static sanity, or a focused smoke test.
- Once relevant checks pass and the implementation appears production-quality against the user's stated goals, such as extensibility, reliability, security, or performance, ask whether they want an independent agent to perform a third-party adversarial review. Do not ask while known production-quality gaps remain.
