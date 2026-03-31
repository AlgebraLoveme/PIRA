# MAINTENANCE

## Maintenance Rules
- `MEMORY.md` is local-only storage and may contain sensitive context.
- If the user requests an insensitive memory update, write it to `~/agent/assets/MEMORY_INIT.md`; use `MEMORY.md` only for local-only context, including sensitive context only if the user explicitly wants that.
- Overwrite with current policy text only: no timestamps, question IDs, changelogs, or override chains.
- After every write, check for cross-file conflicts; if any exist, raise them immediately and confirm resolution with the user.
- After each update to agent configuration or instruction files, auto-compact every touched file for brevity and clarity without losing any information or changing meaning.
- After each update to tracked agent configuration or setup files, audit the touched tracked files for unexpected personal information.
- After each update to agent configuration or instruction files, verify that each rule matches its intended scope. If a more general rule is helpful, generalize it deliberately, define its scope explicitly, and ensure it still matches the user's intent.
- Place rules in the proper location based on their content and intended scope: default/session-wide behavior in auto-loaded files, setup or seed guidance in tracked templates or seed files, and local-only or sensitive context in local-only files.
