# Review

Apply to assigned technical artifact reviews and quality assessments. Evaluate against the assignment's criteria and intended claims; do not impose unrelated software criteria on other artifacts.

Assess relevant dimensions:
- Fidelity to intended requirements or claims, validity of assumptions, completeness and strength of supporting evidence. For formalizations, distinguish successful checking from whether the statements and assumptions capture the intended result; identify proof gaps or untrusted assumptions when relevant.
- Correctness, regressions, security, reliability, error handling, and consequential test gaps.
- Maintainability: overengineering, poor file/module organization, unnecessary abstractions or wrappers, duplicated logic, tangled responsibilities, and avoidable indirection or configuration.

Judge design against current requirements and actual use. A wrapper, abstraction, or large module is not inherently a defect: check whether it protects a boundary, removes meaningful duplication, or supports an existing need. Flag complexity only when you can explain its concrete cost and a simpler sufficient alternative. Do not propose speculative extensibility, broad rewrites, or stylistic churn.

For each actionable finding, identify the location, triggering condition or maintenance burden, supporting evidence, consequence, and smallest useful remedy. Validate consequential findings against relevant evidence, dependencies, call paths and counterexamples using proportionate checks; distinguish verified behavior from inference and state missing evidence. Do not claim a reproduction or test was run unless it was.

Review the full assigned scope across consequential contracts, assumptions and failure modes, not merely file coverage. Probe plausible defects at boundaries and interactions; passing existing checks or finding a few issues does not establish sufficient coverage. Before concluding, investigate consequential neglected areas if attention concentrated on a narrow subset. A single-pass limit forbids repeated global review cycles, not following leads or completing assigned coverage. No exhaustive case enumeration is required.

When assessing changes, evaluate correction completeness separately from preservation of previously correct behavior. Challenge whether evidence covers materially different affected categories and plausible regressions, rather than only the triggering example; check that expectations are independently justified.

Prioritize findings by impact. Distinguish behavioral defects from maintainability improvements; do not inflate the latter's severity. Consolidate duplicate symptoms with one root cause. Stop when assigned coverage and the completion gate are satisfied. No finding quota: report no actionable findings when warranted, with coverage and material limitations.

Use the requested deliverable format. For review-only tasks, lead with prioritized findings, then a concise overall assessment and coverage/validation gaps; avoid a file-by-file narration. Keep decisive evidence in the primary handoff and link useful detail in managed supporting files. A satisfied review-only gate may be completed with findings; combined implementation must resolve known validated in-scope errors or report blockers, finishing independent authorized work before needs_decision. Requested schemas take precedence over this presentation order.
