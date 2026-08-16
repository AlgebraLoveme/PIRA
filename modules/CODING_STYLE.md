# CODING_STYLE

## Requests
- **Implement X:** deliver the smallest complete implementation with sufficient scope-specific checks.
- **Optimize X:** start from existing code; make behavior-preserving refactors for readability and in-scope extensibility; improve reliability, security, resource efficiency, or measured speed.

Executed checks bound claims that code works; report exact gaps. Other coding tasks use applicable rules.

## Workflow
1. Define scope and the smallest useful acceptance check.
2. First sufficient rung: omit unnecessary code (briefly say so) → standard library → native platform feature → cleanly suitable installed dependency → one clear line → minimum working code.
3. Make the minimal safe change: correct, boring, readable over clever/speculative; deletion over addition when possible; the fewest-file, shortest working diff.
4. Run the smallest relevant checks; report gaps.

Before non-trivial refactoring, protect moved behavior with the smallest check. Refactor only to improve in-scope readability or required extensibility, reduce current-change risk, remove duplication, clarify a boundary, or materially ease testing. **Optimize** must prefer behavior-preserving, proportionate readability and extensibility refactoring; any behavior change requires explicit user authorization. Optimize performance only with profiling, measurement, or clear workload evidence; stop when evidence is unconvincing. Briefly comment non-obvious tradeoffs.

## Change Discipline
- Use this global style unless trusted repository-local instructions specify otherwise; explicit user instructions override both.
- Avoid unrequested abstractions, boilerplate, future scaffolding, and configuration for constants.
- Keep data flow explicit and side effects narrow. Keep each function at one abstraction level; extract only to name a real idea, remove duplication, or expose a boundary.
- Avoid flag arguments that create distinct behaviors; split behavior or use an explicit mode only when simpler. Centralize true configuration; avoid scattered hardcoded constants.
- For a complex request, implement a simpler sufficient solution and briefly name omissions; ask only when defaulting is risky.
- When cheap, isolate stable core logic from volatile CLI, file I/O, network, database, UI, framework, and subprocess details. Dependencies point from volatile outer code to stable inner logic; core logic must not import infrastructure merely for convenience.
- Pass simple data across boundaries; do not leak framework, ORM, request, or process objects into core logic unless the project is intentionally glue code.
- Improve boundaries incrementally within touched code. Do not make architecture-wide or drive-by refactors; leave in-scope touched code slightly cleaner.

## Names, Types, and Dependencies
- Use type hints when appropriate, especially on function/method signatures.
- Names reveal intent, domain meaning, units, and important distinctions. Use one word per concept; avoid misleading near-synonyms. Keep names concise unless expansion removes ambiguity; propose one best name by default.
- Add a dependency only for clear material benefit when owning a few lines would be worse. Between equally small standard-library/platform options, choose better edge-case correctness.
- For large likely-open-source features, survey high-quality online implementations; raise promising options and confirm with the user.

## Contracts, Failures, and Security
- Never simplify away trust-boundary validation, data-loss-preventing error handling, security behavior, accessibility basics, or real-hardware calibration knobs.
- Security is behavior: preserve in-scope authentication, authorization, permission/scope checks, secret handling, safe parsing/escaping, injection/XSS/CSRF/SSRF protections, resource limits, crypto/TLS defaults, and audit-relevant logs.
- Add runtime checks only where strict assumptions matter (e.g. shape, range, dtype, device, trust, or security boundaries). Checks must be narrow, fail fast, and be actionable; avoid silent fallback unless explicitly requested.
- Keep error paths visible without obscuring main flow. Swallow/translate errors only to add actionable context. A failure/exception bug fix requires the smallest practical failure-path check.
- Mark intentional simplifications with `PIRA:`. For a known shortcut ceiling (e.g. global lock, $O(n^2)$ scan, naive heuristic), name the ceiling and upgrade path.

## Operability
- Default to concise structured logs for configuration, major-stage start/end, and critical metrics; avoid per-iteration logs unless explicitly debugging.
- Public APIs need concise docstrings; internal/helper docstrings only for non-obvious logic. Prefer clearer names/structure to comments; comments cover intent, invariants, assumptions, ceilings, and tradeoffs—not obvious syntax.
- For non-obvious tensor-shape handling, infer and note shapes inline; run small tests when needed to confirm important shapes.
- For new stochastic Python workflows without a project convention, add centralized `seed_everything(seed)` by default. In other languages or established projects, follow the local convention. Add no further reproducibility metadata unless explicitly requested.

## Verification
- Non-trivial new logic needs the smallest runnable check that fails if it breaks; trivial one-liners need no tests.
- Tests must be readable, independent, fast, and focused on observable behavior, not implementation shape.
- Run user-specified tests first; otherwise use minimal fast checks: syntax, grammar, static sanity, or a focused smoke test.
