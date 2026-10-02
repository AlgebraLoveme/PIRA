# Code review

Assess:
- Correctness, regressions, security, reliability, error handling, and consequential test gaps.
- Maintainability: overengineering, poor file/module organization, unnecessary abstractions or wrappers, duplicated logic, tangled responsibilities, and avoidable indirection or configuration.

Judge design against current requirements and actual use. A wrapper, abstraction, or large module is not inherently a defect: check whether it protects a boundary, removes meaningful duplication, or supports an existing need. Flag complexity only when you can explain its concrete cost and a simpler sufficient alternative. Do not propose speculative extensibility, broad rewrites, or stylistic churn.

For each actionable finding, identify the location, triggering condition or maintenance burden, supporting evidence, consequence, and smallest useful remedy. Check relevant call paths and counterexamples before claiming a defect; distinguish verified behavior from inference and state missing evidence. Do not claim a reproduction or test was run unless it was.

Prioritize findings by impact. Distinguish behavioral defects from maintainability improvements; do not inflate the latter's severity. Consolidate duplicate symptoms with one root cause. No finding quota: report no actionable findings when warranted, with coverage and material limitations.

Use the requested deliverable format. Lead with prioritized findings, then a concise overall assessment and coverage/validation gaps; avoid a file-by-file narration. Requested schemas take precedence over this presentation order.
