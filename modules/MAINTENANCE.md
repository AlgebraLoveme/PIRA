# MAINTENANCE

Use this module only for maintenance of PIRA's own configuration, modules, and rules, not for ordinary project work.

## Maintenance Rules
- Keep workbook-behavior defaults in `~/agent/AGENTS.md`; use this module only to maintain those defaults, not to define ordinary workbook handling locally.
- When files are removed from the active scheme in the future, add them to `~/agent/assets/LEGACY_LIST.md`.
- Overwrite with current policy text only: no timestamps, question IDs, changelogs, or override chains.
- After every write, check for cross-file conflicts and raise any immediately.
- After updating agent configuration or instruction files, auto-compact the touched files without losing meaning, audit tracked touched files for unexpected personal information, and verify that each rule still matches its intended scope; if a more general rule would help, generalize it deliberately, define its scope explicitly, and ensure it still matches the user's intent.
- When a new experimental rule conflicts with an older established rule, preserve the older default unless the user explicitly approves a scope or routing change.
- Put rules in the proper place: default/session-wide behavior in auto-loaded files, setup guidance in tracked templates or seed files, and local-only or sensitive context in local-only files.
- Keep module-loading and routing policy only in `~/agent/AGENTS.md`.
- After commit and push, clean temporary backup files created for the change if they are no longer needed.
