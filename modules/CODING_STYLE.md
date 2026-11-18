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
- Among routes that reliably satisfy the clarified behavior requirements and safety constraints, always prefer, in order:
  1. Simpler implementation, judged by expected implementation difficulty, then expected code length, then the expected number of code files requiring edits.
  2. Greater capability or scalability when the simplicity criteria above are similar.
  3. Fewer dependencies when the first two criteria are similar.
- When a technical choice materially affects result quality or performance and the intended tradeoff is unclear, clarify design intent before choosing. Briefly explain the relevant routes, their consequences, and differences. Ask about the desired outcome, not the technical choice.
- Actively look for harmless assumptions that hold in almost all intended use cases and greatly simplify implementation. An assumption is eligible only when avoiding it creates unavoidable implementation difficulty. Establish that difficulty from concrete constraints and reasonable assumption-free alternatives, rather than convenience or speculative complexity. Assess accidental mistakes as well as ordinary use when judging harmlessness.
- Before adopting an eligible assumption, ask the user to confirm it. Explain why it should usually hold, concrete cases where it could fail and their consequences, and how much complexity it removes. Keep assumption-dependent implementation pending until explicit confirmation; continue independent work. Never treat likely truth or implementation convenience as approval.

## Workflow
1. Define scope and the smallest useful acceptance check.
2. Omit unnecessary code and briefly say so; otherwise choose the smallest sufficient route under the decision order above.
3. Make the minimal safe change: correct, boring, readable over clever or speculative; prefer deletion when possible and the fewest-file, shortest working diff.
4. Verify under the rules below; report gaps.

Before non-trivial refactoring, protect moved behavior with the smallest check. Refactor only for in-scope readability, required extensibility, reduced current-change risk or duplication, clearer boundaries, or materially easier testing. **Optimize** must prefer proportionate, behavior-preserving readability and extensibility refactoring; behavior changes require explicit user authorization. Do not add complexity for marginal gains unless explicitly requested. Optimize performance only with profiling, measurement, or clear workload evidence; stop when evidence is unconvincing. Briefly note non-obvious tradeoffs.

## Change Discipline
- Preserve compatibility when it is straightforward and low-cost. When retaining it would have serious consequences or substantially complicate runtime logic, move the compatibility transition into explicit setup or migration scripts instead. Preserve data and make ambiguous or unsafe migrations fail visibly; this does not authorize unrelated breaking changes.
- Use this global style unless trusted repository-local instructions specify otherwise; explicit user instructions override both.
- Avoid unrequested abstractions, boilerplate, future scaffolding, and configuration for constants.
- Keep data flow explicit, side effects narrow, and one abstraction level per function. Extract only to name a real idea, remove duplication, or expose a boundary.
- Avoid flag arguments creating distinct behaviors; split behavior or use an explicit mode only when simpler. Centralize true configuration; avoid scattered hardcoded constants.
- When cheap, isolate stable core logic from volatile infrastructure (CLI, I/O, network, database, UI, frameworks, subprocesses); dependencies point inward. Pass simple data across boundaries. Core logic must not import infrastructure merely for convenience. Do not leak infrastructure objects into core logic unless the project is intentionally glue code.
- Improve boundaries incrementally in touched code, leaving it slightly cleaner. Do not make architecture-wide or drive-by refactors.

## Names, Types, and Dependencies
- Use type hints when appropriate, especially in function/method signatures.
- Names reveal intent, domain meaning, units, and important distinctions. Use one word per concept, not misleading near-synonyms. Stay concise unless expansion removes ambiguity; propose one best name by default.
- Prioritize useful, reliable behavior over minimizing dependencies. When the promised functionality requires a dependency for reliable execution, require it rather than maintain a partial substitute solely to avoid that dependency. If unavailable, report the requirement and an actionable setup path; offer reduced functionality only as a separately defined, explicitly selected contract. Among approaches meeting the same contract, apply the decision order above; between otherwise comparable standard-library/platform options, choose better edge-case correctness.
- For large, likely open-source features, survey high-quality online implementations and raise promising options. Confirm design-level tradeoffs under the ownership rules; choose technical details independently.

## Contracts, Failures, and Security
- Preserve trust-boundary validation, data-loss-preventing error handling, security behavior, accessibility basics, and real-hardware calibration controls required by the agreed behavior and threat model. Proposed changes to those requirements follow the assumption-approval gate; an honest-agent assumption never means agents are mistake-free.
- Preserve in-scope authentication, authorization, permission and scope checks, secret handling, safe parsing and escaping, injection/XSS/CSRF/SSRF protections, resource limits, crypto/TLS defaults, and audit-relevant logs.
- Add runtime checks only where strict assumptions matter (e.g., shape, range, dtype, device, trust, security boundaries). Checks must be narrow, fail fast, and actionable; avoid silent fallback unless explicitly requested.
- Keep error paths visible without obscuring the main flow. Swallow/translate errors only to add actionable context. Failure/exception fixes require the smallest practical failure-path check.
- Mark intentional simplifications with `PIRA:`. For a known shortcut ceiling, name the ceiling and upgrade path.

## Operability
- Default to concise structured logs for configuration, major-stage start/end, and critical metrics; avoid per-iteration logs except in explicit debugging.
- Public APIs need concise docstrings; internal/helper docstrings only for non-obvious logic. Prefer clear names/structure to comments, which explain intent, invariants, assumptions, ceilings, and tradeoffs, not obvious syntax.
- For non-obvious tensor-shape handling, infer and note shapes inline; use small tests when needed to confirm important shapes.
- For new stochastic Python workflows without a project convention, default to adding centralized `seed_everything(seed)`. Otherwise follow language or project convention. Add no further reproducibility metadata unless requested.

## Test-case writing
- Test the module through the interface its callers use. The suite must cover every distinct behavior that interface promises. Identify those behaviors, check which the existing tests already cover, and add cases for the gaps. Do not write a separate test for each helper, class definition or file.
- Decide what should happen before choosing the assertion. For specified behavior, use a known answer, a requirement you can check, or a trusted reference. Do not run the new code once and copy its output into the test as the expected answer.
- If two versions, paths or conditions should give the same result, exercise both with matching inputs and compare what callers receive or can do afterward. For example, after moving records to a new format, read them through the module and compare their contents; do not require identical file bytes unless that too is promised. Check intentional differences against the new requirement, not against the old result.
- Once ordinary use is covered, prefer cases that could reveal a different mistake: special values, allowed extremes, state changes or interactions. For an inclusive upper limit, test the limit itself and the first value above it when both acceptance and rejection matter. Skip extra inputs that exercise the same rule without testing a new risk.
- Check more than the return value when the interface promises more. For example, if a failed update must preserve old data, try reading the old data afterward. If callers should have read-only access, try changing the data through the access they actually receive and check that the module’s state stays unchanged.
- Give each case one purpose. When testing a rejection, make the other inputs valid so the wrong check cannot make the test pass. Group cases that differ only in input and expectation when each failure remains identifiable.
- Use small, independent setups that exercise the real module; do not replace the behavior under test with a fake. Avoid repeating detailed checks at multiple levels. Prefer simple setup over elaborate shared helpers. Keep tests fast and repeatable. In asynchronous tests, wait for the observable condition the test needs, with a deadline so a missing event fails the test rather than hanging it.

## Verification
- Verify every implementation, but add or update lasting tests only for consequential gaps in existing coverage. Identify the changed contract, what must still work and which plausible failure matters. Run user-specified checks first; otherwise start with the smallest runnable check that can establish the claim. Complete all required checks.
- For a fix, verify both the correction and preservation of affected, previously correct behavior. When practical, reproduce the original failure for the intended reason and show it passes after the fix; use a materially different case when needed to rule out an input-specific patch.
- Match the check to the claim. Use real component, process or platform interaction when the contract depends on it. Use fault injection, process-death, stress or combinatorial checks only when simpler checks cannot establish an important property.
- Once required checks and affected regressions pass, broaden or repeat only for changed behavior, a new failure or a specific consequential gap. Report material unverified requirements. Test counts, source length and coverage percentages alone do not establish sufficiency.
- Once relevant checks pass and the implementation appears production-quality against the user's stated goals (e.g., extensibility, reliability, security, performance), ask whether they want an independent third-party adversarial agent review. Do not ask while known production-quality gaps remain.
