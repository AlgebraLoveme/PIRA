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
- After commit and push, clean temporary backup files created for the change if they are no longer needed.
