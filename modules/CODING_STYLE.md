# CODING_STYLE

## Requests
- **Implement X:** deliver the smallest complete implementation. Complete means the user's goal is fully satisfied and verified with sufficient, non-excessive checks under Verification. Stop and seek user confirmation if the only remaining steps are highly consequential with intent unclear from the user's request, expensive, or potentially destructive. The implementation remains incomplete while any work needed to satisfy or verify the user's goal can proceed without user confirmation.
- **Optimize X:** start from existing code; use behavior-preserving refactors for readability and in-scope extensibility; improve reliability, security, resource efficiency, or measured speed.

Executed checks bound claims that code works; report exact gaps. Other coding tasks use relevant rules.

## Design and Technical Decision Ownership
- The user owns design-level decisions: intended outcomes, scope, externally visible behavior, and acceptable outcome tradeoffs. The agent assists with well-thought-out recommendations that explain the relevant alternatives and consequences.
- When a previously confirmed design clearly no longer fits the current requirements or scenario and a clearly better alternative exists, immediately propose the design change rather than accumulating fixes to compensate for the design failure. Explain the mismatch and the alternative's benefits and tradeoffs; obtain user confirmation before changing the design.
- When context clearly shows that the user is developing a system or system component and design choices need to be recorded, draft the design in a workspace Markdown file instead of presenting it in full in the conversation. Keep the document minimal but sufficient for agent implementation and human understanding. Include only details needed to understand or implement the intended design; omit stale, superseded, or otherwise unnecessary details. Distinguish proposals from confirmed decisions; give the file path and direct the user to read it.
- The agent owns technical-level decisions, using best-expert knowledge and sufficient clarification of the user's goals and constraints. Do not ask the user to select implementation details when the design intent is clear.
- Among routes that satisfy the clarified requirements and safety constraints, always prefer, in order:
  1. Simpler implementation, judged by expected implementation difficulty, then expected code length, then the expected number of code files requiring edits.
  2. Greater capability or scalability when the simplicity criteria above are similar.
  3. Fewer dependencies when the first two criteria are similar.
- When a technical choice materially affects result quality or performance and the intended tradeoff is not clear from context, clarify the design intent with the user before choosing. Briefly describe the relevant implementation routes, emphasizing their consequences and why they differ. Ask about the desired outcome rather than delegating the technical choice.

## Workflow
1. Define scope and the smallest useful acceptance check.
2. Omit unnecessary code and briefly say so; otherwise choose the smallest sufficient route under the decision order above.
3. Make the minimal safe change: correct, boring, readable over clever or speculative; prefer deletion when possible and the fewest-file, shortest working diff.
4. Verify under the rules below; report gaps.

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
- Evaluate dependencies under the decision order above; add one only for clear material benefit over owning the required code. Between otherwise comparable standard-library/platform options, choose better edge-case correctness.
- For large, likely open-source features, survey high-quality online implementations and raise promising options. Seek user confirmation for design-level tradeoffs under the ownership rules above; choose technical details independently.

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
- Write tests only when they are meaningful and necessary to verify the implementation. Do not write implementation-mirroring tests for reversible, low-impact changes. Tests must be readable, independent, fast, and focused on observable behavior rather than implementation shape.
- For medium- or high-level behavior, include the smallest diverse set of boundary tests that provides most of the assurance. Prefer cases that exercise multiple boundaries at once.
- Run tests appropriate to the change and complete required checks. Run user-specified tests first; otherwise start with minimal fast checks such as syntax, grammar, static sanity, or a focused smoke test.
- Once appropriate tests and required checks pass, broaden or repeat testing only when new changes, failures, or unresolved concerns justify it; otherwise continue toward completing the task.
- Once relevant checks pass and the implementation appears production-quality against the user's stated goals, such as extensibility, reliability, security, or performance, ask whether they want an independent agent to perform a third-party adversarial review. Do not ask while known production-quality gaps remain.
