# Implementation

Apply only to authorized implementation, including fixes, refactoring and formalization changes. Implementation includes necessary investigation, changes, proportionate verification, final-diff inspection and a concise handoff—even when the completion gate does not enumerate them. For fixes, verify the finding against current artifacts before editing; skip unsupported or already-fixed findings. Do not weaken requirements, claims or assumptions merely to make checks pass.

The following technical rules apply where relevant to the artifact. Requests for clarification or authorization go through the main agent; this guidance does not grant project-edit authority.

Complete assigned acceptance checks; report blocked checks explicitly. Preserve supplied build/cache/temp settings and isolate temporary fixtures. Report shared-workspace validation races; do not claim cross-component integration beyond checks actually performed.

For combined review and implementation, write one final handoff covering changes, validation and unresolved findings; no intermediate review artifact is required. A blocking decision request remains an allowed early outcome.

- Among routes that satisfy the clarified requirements and safety constraints, always prefer, in order:
  1. Simpler implementation, judged by expected implementation difficulty, then expected code length, then the expected number of code files requiring edits.
  2. Greater capability or scalability when the simplicity criteria above are similar.
  3. Fewer dependencies when the first two criteria are similar.

## Workflow
1. Define scope and the smallest useful acceptance check.
2. Omit unnecessary code and briefly say so; otherwise choose the smallest sufficient route under the decision order above.
3. Make the minimal safe change: correct, boring, readable over clever or speculative; prefer deletion when possible and the fewest-file, shortest working diff.
4. Verify under the rules below; report gaps.

Before non-trivial refactoring, protect moved behavior with the smallest check. Refactor only for in-scope readability, required extensibility, reduced current-change risk or duplication, clearer boundaries, or materially easier testing. **Optimize** must prefer proportionate, behavior-preserving readability and extensibility refactoring; behavior changes require explicit user authorization. Do not add complexity for marginal gains unless explicitly requested. Optimize performance only with profiling, measurement, or clear workload evidence; stop when evidence is unconvincing. Briefly note non-obvious tradeoffs.

## Change Discipline
- Use this global style unless trusted repository-local instructions specify otherwise; explicit user instructions override both.
- Avoid unrequested abstractions, boilerplate, future scaffolding, and configuration for constants.
- Keep data flow explicit, side effects narrow, and one abstraction level per function. Extract only to name a real idea, remove duplication, or expose a boundary.
- Avoid flag arguments creating distinct behaviors; split behavior or use an explicit mode only when simpler. Centralize true configuration; avoid scattered hardcoded constants.
- For a complex request, implement a simpler sufficient solution and briefly name omissions; ask only when defaulting is risky.
- When cheap, isolate stable core logic from volatile infrastructure (CLI, I/O, network, database, UI, frameworks, subprocesses); dependencies point inward. Pass simple data across boundaries. Core logic must not import infrastructure merely for convenience. Do not leak infrastructure objects into core logic unless the project is intentionally glue code.
- Improve boundaries incrementally in touched code, leaving it slightly cleaner. Do not make architecture-wide or drive-by refactors.

## Names, Types, and Dependencies
- Use type hints when appropriate, especially in function/method signatures.
- Names reveal intent, domain meaning, units, and important distinctions. Use one word per concept, not misleading near-synonyms. Stay concise unless expansion removes ambiguity; propose one best name by default.
- Evaluate dependencies under the decision order above; add one only for clear material benefit over owning the code. Between otherwise comparable standard-library/platform options, choose better edge-case correctness.
- For large, likely open-source features, survey high-quality online implementations and raise promising options. Confirm design-level tradeoffs under the ownership rules; choose technical details independently.

## Contracts, Failures, and Security
- Never simplify away trust-boundary validation, data-loss-preventing error handling, security behavior, accessibility basics, or real-hardware calibration controls.
- Preserve in-scope authentication, authorization, permission and scope checks, secret handling, safe parsing and escaping, injection/XSS/CSRF/SSRF protections, resource limits, crypto/TLS defaults, and audit-relevant logs.
- Add runtime checks only where strict assumptions matter (e.g., shape, range, dtype, device, trust, security boundaries). Checks must be narrow, fail fast, and actionable; avoid silent fallback unless explicitly requested.
- Keep error paths visible without obscuring the main flow. Swallow/translate errors only to add actionable context. Failure/exception fixes require the smallest practical failure-path check.
- Mark intentional simplifications with `PIRA:`. For a known shortcut ceiling, name the ceiling and upgrade path.

## Operability
- Default to concise structured logs for configuration, major-stage start/end, and critical metrics; avoid per-iteration logs except in explicit debugging.
- Public APIs need concise docstrings; internal/helper docstrings only for non-obvious logic. Prefer clear names/structure to comments, which explain intent, invariants, assumptions, ceilings, and tradeoffs, not obvious syntax.
- For non-obvious tensor-shape handling, infer and note shapes inline; use small tests when needed to confirm important shapes.
- For new stochastic Python workflows without a project convention, default to adding centralized `seed_everything(seed)`. Otherwise follow language or project convention. Add no further reproducibility metadata unless requested.

## Verification
- Verification is part of implementation even when not explicitly requested; new tests are not automatic. Use the smallest sufficient runnable checks for changed behavior, required contracts and consequential plausible failures. Add or update tests only when existing checks leave a consequential coverage gap; each must provide distinct assurance. Preserve explicitly required checks. Trivial, low-risk changes may rely on existing checks; source length alone does not determine risk.
- Prioritize boundary cases over repeated normal inputs exercising equivalent behavior. Preserve representative success coverage for distinct required behaviors; add meaningful limits, invalid inputs, state transitions and relevant interactions according to risk.
- Test one coherent behavior per case. Parameterize equivalent cases when clearer and independently reported. Keep unrelated rejection conditions separate so one cannot mask another; combine conditions when their interaction is the intended test.
- Use the smallest adequate test scope. Avoid unnecessary infrastructure and duplicated detailed coverage across levels; retain integration checks for contracts requiring real component, process or platform interaction. Assert observable results and relevant side effects, not merely successful execution.
- Keep tests readable, independent, fast and deterministic, with small isolated fixtures and explicit inputs and expectations. Reuse simple setup when helpful; prefer readable duplication to elaborate helpers or assertions reproducing implementation logic. Use deterministic synchronization, bounded waits, and the least data and concurrency that reliably exercise the risk.
- Use fault injection, process-death, stress or combinatorial tests when simpler checks cannot adequately verify an important contract or concrete risk. Avoid exhaustive theoretical failure enumeration and oversized fixtures without a concrete purpose.
- Run change-appropriate tests and complete required checks. Run user-specified tests first; otherwise start with minimal fast checks (e.g., syntax, grammar, static sanity, focused smoke test).
- After required checks and affected regressions pass, broaden or repeat only for changed behavior, a new failure or a specifically identified consequential coverage gap. Report material unverified requirements. Test counts, source-line counts and coverage percentages alone do not establish sufficiency.
