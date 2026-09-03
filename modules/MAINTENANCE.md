# MAINTENANCE

Use only for PIRA configuration, module, and rule maintenance—not ordinary project work.

## Maintenance Rules
- Keep memory defaults in `Memory System` in the active Claude PIRA source checkout's `AGENTS.md`; this module maintains them but does not define ordinary memory handling. Refresh the installed snapshot afterward.
- Add files removed from the active scheme to `assets/LEGACY_LIST.md` in the active Claude PIRA source checkout.
- Overwrite with current policy only: no timestamps, question IDs, changelogs, or override chains.
- After every maintenance write, immediately check and raise cross-file conflicts. For instruction or agent-configuration updates, remove introduced or obsolete redundancy without changing unrelated meaning; audit tracked touched files for unexpected personal information and every rule's intended scope. Generalize only deliberately and within the user's intended scope.
- When an experimental rule conflicts with an established rule, preserve the established default unless the user explicitly approves a scope or routing change.
- Place default or session-wide behavior in auto-loaded files, setup guidance in tracked templates or seed files, and local-only or sensitive context in local-only files.
- Keep module loading and routing only in the Claude branch's source `AGENTS.md`; `~/.claude/pira/AGENTS.md` is an installed snapshot, not an editing target.
- Keep README public-phase only: public behavior, releases, artifacts, usage, and reproducible public validation—not local development candidates, pending work, private validation state, or rollout plans.
- After commit and push, remove obsolete temporary backups created for the change.

## Meaning-Preserving Telegraphic Compression (MPTC)

Use MPTC when shortening PIRA instructions:

- Preserve every rule's actor, modality, trigger, action, object, scope, ordering, exceptions, and prohibitions. Never weaken, strengthen, broaden, or narrow it.
- Remove filler, repeated context, and details fully covered by tool help. Merge only genuinely parallel rules.
- Preserve normative strength exactly: `must`, `should`, `use`, `prefer`, `may`, `never`, and `only` are not interchangeable. Never reduce a requirement to `prefer`, `avoid`, or an implicit fragment.
- Use fragments and symbols only for low-risk routing or sequencing with an obvious actor and trigger: `→` means next step, `⇒` consequence, and `+` both. Never use symbols for permission, prohibition, safety, exceptions, or normative strength.
- Keep grammatical sentences for permission, safety, trust boundaries, destructive actions, negation, and exceptions. Clarity outranks compression.
- Validate every original rule one-to-one against `actor | modality | trigger | action | object | scope | order | exception | prohibition`; expand if any field is lost, changed, or reasonably debatable.
- Measure and report compression only after fidelity validation passes. State the metric and label token savings as estimates unless measured with the target tokenizer.
