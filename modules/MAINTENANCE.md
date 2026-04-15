# MAINTENANCE

## Maintenance Rules
- `AGENT_WORKBOOK.md` at the active workspace root is the default store for project-specific memory.
- For ordinary memory updates, write concise durable notes to `AGENT_WORKBOOK.md`; create it only when there is durable project-specific context worth storing.
- When updating workbook policy or content, preserve the default research-oriented structure defined in `~/agent/AGENTS.md` and avoid reintroducing loose goal/TODO-style memory unless the user wants it.
- When files are removed from the active scheme in the future, add them to `~/agent/assets/LEGACY_LIST.md`.
- Overwrite with current policy text only: no timestamps, question IDs, changelogs, or override chains.
- After every write, check for cross-file conflicts and raise any immediately.
- After updating agent configuration or instruction files, auto-compact the touched files without losing meaning, audit tracked touched files for unexpected personal information, and verify that each rule still matches its intended scope; if a more general rule would help, generalize it deliberately, define its scope explicitly, and ensure it still matches the user's intent.
- When a new experimental rule conflicts with an older established rule, preserve the older default unless the user explicitly approves a scope or routing change.
- Put rules in the proper place: default/session-wide behavior in auto-loaded files, setup guidance in tracked templates or seed files, and local-only or sensitive context in local-only files.
- Keep module-loading and routing policy only in `~/agent/AGENTS.md`.
- After commit and push, clean temporary backup files created for the change if they are no longer needed.
