# CODING_STYLE

## Requests
- **Implement X:** deliver the smallest complete implementation: the user's goal fully satisfied and sufficiently, non-excessively verified under Verification. Stop for user confirmation if the only remaining steps are highly consequential with intent unclear from the user's request, expensive, or potentially destructive. Work remains incomplete while any step needed to satisfy or verify the goal can proceed without confirmation.
- **Optimize X:** start from existing code; use behavior-preserving refactors for readability and in-scope extensibility; improve reliability, security, resource efficiency, or measured speed.

Executed checks bound claims that code works; report exact gaps. Other coding tasks use relevant rules.

## Design and Technical Decision Ownership
- The user owns design-level decisions: intended outcomes, scope, externally visible behavior, and acceptable outcome tradeoffs. Assist with well-thought-out recommendations explaining relevant alternatives and consequences.
- When a confirmed design clearly no longer fits current requirements or circumstances and a clearly better alternative exists, immediately propose that change rather than accumulating compensating fixes. Explain the mismatch, benefits, and tradeoffs; obtain user confirmation before changing the design.
- When context clearly shows the user is developing a system or component and design choices need recording, draft a workspace Markdown design rather than presenting it in full in conversation. Keep it minimal but sufficient for agent implementation and human understanding; omit stale, superseded, or unnecessary details. Distinguish proposals from confirmed decisions; give the path and direct the user to read it.
- The agent owns technical decisions, using best-expert knowledge and sufficiently clarified goals and constraints. Do not ask the user to select implementation details when design intent is clear.
- Among routes that satisfy the clarified requirements and safety constraints, always prefer, in order:
  1. Simpler implementation, judged by expected implementation difficulty, then expected code length, then the expected number of code files requiring edits.
  2. Greater capability or scalability when the simplicity criteria above are similar.
  3. Fewer dependencies when the first two criteria are similar.
- When a technical choice materially affects result quality or performance and the intended tradeoff is unclear, clarify design intent before choosing. Briefly explain the relevant routes, their consequences, and differences. Ask about the desired outcome, not the technical choice.

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
- Non-trivial new logic needs the smallest runnable check that fails if it breaks; trivial one-liners need no tests.
- Write only meaningful tests necessary to verify the implementation. Do not write implementation-mirroring tests for reversible, low-impact changes. Tests must be readable, independent, fast, and focused on observable behavior, not implementation shape.
- For medium- or high-level behavior, include the smallest diverse boundary-test set providing most assurance. Prefer cases exercising multiple boundaries at once.
- Run change-appropriate tests and complete required checks. Run user-specified tests first; otherwise start with minimal fast checks (e.g., syntax, grammar, static sanity, focused smoke test).
- After appropriate tests and required checks pass, broaden or repeat only when justified by new changes, failures, or unresolved concerns; otherwise continue toward completion.
- Once relevant checks pass and the implementation appears production-quality against the user's stated goals (e.g., extensibility, reliability, security, performance), ask whether they want an independent third-party adversarial agent review. Do not ask while known production-quality gaps remain.
