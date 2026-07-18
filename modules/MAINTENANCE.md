# MAINTENANCE

Use this module only for maintenance of PIRA's own configuration, modules, and rules, not for ordinary project work.

## Maintenance Rules
- Keep memory-system defaults in `~/agent/MEMORY_SYSTEM.md`; use this module only to maintain them, not to define ordinary memory handling locally.
- When files are removed from the active scheme in the future, add them to `~/agent/assets/LEGACY_LIST.md`.
- Overwrite with current policy text only: no timestamps, question IDs, changelogs, or override chains.
- After every write, check for cross-file conflicts and raise any immediately.
- After updating agent configuration or instruction files, remove redundancy introduced or made obsolete by that change without rewriting unrelated meaning; then audit cross-file conflicts, tracked touched files for unexpected personal information, and each rule's intended scope. Generalize a rule only deliberately, with explicit scope that still matches the user's intent.
- When a new experimental rule conflicts with an older established rule, preserve the older default unless the user explicitly approves a scope or routing change.
- Put rules in the proper place: default/session-wide behavior in auto-loaded files, setup guidance in tracked templates or seed files, and local-only or sensitive context in local-only files.
- Keep module-loading and routing policy only in `~/agent/AGENTS.md`.
- Keep README public-phase only: document public behavior, releases, artifacts, usage, and reproducible public validation; exclude local development candidates, pending work, private validation state, and rollout plans.
- After commit and push, clean temporary backup files created for the change if they are no longer needed.

## Meaning-Preserving Telegraphic Compression

Use **Meaning-Preserving Telegraphic Compression (MPTC)** when shortening PIRA instructions:

- Preserve each rule's actor, modality, trigger, action, object, scope, ordering, exceptions, and prohibitions. Compression must neither weaken nor strengthen, broaden nor narrow the rule.
- Remove filler, repeated context, and details already fully covered by tool help. Merge only genuinely parallel rules.
- Preserve normative strength exactly: `must`, `should`, `use`, `prefer`, `may`, `never`, and `only` are not interchangeable. Never turn a requirement into `prefer`, `avoid`, or an implicit fragment.
- Use fragments and symbols only for low-risk routing or sequencing with an obvious actor and trigger: `→` = next step, `⇒` = consequence, and `+` = both. Do not use symbols to encode permission, prohibition, safety, exceptions, or normative strength.
- Keep grammatical sentences for permission, safety, trust boundaries, destructive actions, negation, and exceptions. Clarity outranks compression.
- Validate every original rule one-to-one against `actor | modality | trigger | action | object | scope | order | exception | prohibition`. If any field is lost, changed, or reasonably debatable, expand the compressed rule.
- Measure and report compression only after fidelity validation passes; identify the metric used and label token savings as estimates unless measured with the target tokenizer.
