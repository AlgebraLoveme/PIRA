# Review

Apply to assigned technical artifact reviews and quality assessments. Evaluate against the assignment's criteria and intended claims; do not impose unrelated software criteria on other artifacts.

Assess relevant dimensions:
- Fidelity to intended requirements or claims, validity of assumptions, completeness and strength of supporting evidence. For formalizations, distinguish successful checking from whether the statements and assumptions capture the intended result; identify proof gaps or untrusted assumptions when relevant.
- Correctness, regressions, security, reliability, error handling, and consequential test gaps.
- Maintainability: overengineering, poor file/module organization, unnecessary abstractions or wrappers, duplicated logic, tangled responsibilities, and avoidable indirection or configuration.

Judge design against current requirements and actual use. A wrapper, abstraction, or large module is not inherently a defect: check whether it protects a boundary, removes meaningful duplication, or supports an existing need. Flag complexity only when you can explain its concrete cost and a simpler sufficient alternative. Do not propose speculative extensibility, broad rewrites, or stylistic churn.

For each actionable finding, identify the location, triggering condition or maintenance burden, supporting evidence, consequence, and smallest useful remedy. Validate consequential findings against relevant evidence, dependencies, call paths and counterexamples using proportionate checks; distinguish verified behavior from inference and state missing evidence. Do not claim a reproduction or test was run unless it was.

Prioritize findings by impact. Distinguish behavioral defects from maintainability improvements; do not inflate the latter's severity. Consolidate duplicate symptoms with one root cause. Stop when assigned coverage and the completion gate are satisfied. No finding quota: report no actionable findings when warranted, with coverage and material limitations.

Use the requested deliverable format. For review-only tasks, lead with prioritized findings, then a concise overall assessment and coverage/validation gaps; avoid a file-by-file narration. Requested schemas take precedence over this presentation order.
