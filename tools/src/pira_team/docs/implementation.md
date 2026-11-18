# Implementation

Apply only to authorized implementation, including fixes, refactoring and formalization changes. Implementation includes necessary investigation, changes, proportionate verification, final-diff inspection and a concise handoff—even when the completion gate does not enumerate them. For fixes, verify the finding against current artifacts before editing; skip unsupported or already-fixed findings. Do not weaken requirements, claims or assumptions merely to make checks pass.

The following technical rules apply where relevant to the artifact. Requests for clarification or authorization go through the main agent; this guidance does not grant project-edit authority.

Complete assigned acceptance checks; report blocked checks explicitly. Preserve supplied build/cache/temp settings and isolate temporary fixtures. Report shared-workspace validation races; do not claim cross-component integration beyond checks actually performed.

For staged review and implementation, use the retained review findings and write one final handoff covering changes, validation and unresolved findings; no polished intermediate review report is required. Pause decision-dependent work, finish independent authorized work, then return needs_decision with the remaining question and completed work/checks. Known validated in-scope unresolved errors required by the fix assignment are blockers, not completed-with-limitations outcomes.

- Among routes that reliably satisfy the clarified behavior requirements and safety constraints, always prefer, in order:
  1. Simpler implementation, judged by expected implementation difficulty, then expected code length, then the expected number of code files requiring edits.
  2. Greater capability or scalability when the simplicity criteria above are similar.
  3. Fewer dependencies when the first two criteria are similar.

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
- Before fixing, trace the affected behavior through relevant callers, consumers, shared state and representations, including dependencies affected by transformation order. Identify the faulty assumption, intended correction, materially different affected categories, and consequential dependent behavior and compatibility constraints. Before preserving existing behavior, distinguish intended compatibility from the defect itself. Use proportionate baseline probes when unclear and escalate outcome-changing ambiguity under the ownership rules.
- Minimize implementation complexity, not correction completeness. Address the root cause across the materially different affected categories, not only the initial reproduction. Discovered residual cases within the intended correction remain unresolved work, not optional scope expansion. A single-pass limit does not prohibit investigating or correcting cases exposed during validation.

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
- For changes to shared logic, compare affected behavior across distinct uses. Investigate unexpected differences; do not weaken assertions or approve changed snapshots merely to make the patch pass.
- Review the final diff against the impact analysis for incomplete correction, unintended changes and effects on dependent paths. Verify consequential risks with targeted checks and resolve newly exposed in-scope issues under the completion rules above. Bound claims to the assessed scope and report material verification gaps; do not claim universal absence of side effects. This is proportionate implementation work, not another global review, a mandatory separate report or another reviewer; do not repeat sufficient checks mechanically.
