# personal_agent setup

Repository: https://github.com/AlgebraLoveme/personal_agent

Goal:
- Configure your coding model to auto-load this policy globally.
- Treat `~/agent/AGENTS.md` and its referenced modules as the source of truth.
- Recommended clone location: `~/agent`.

## Quick start
1. Clone to `~/agent` (recommended): `git clone https://github.com/AlgebraLoveme/personal_agent.git ~/agent`
2. Open your coding model tool (Codex / Claude / others).
3. Paste the prompt below and let the model handle setup automatically, including `USER.md` and `MEMORY.md`.

## Prompt to paste into your model
```
I cloned a repo containing an agent policy folder. Please set up a global agent pipeline automatically. Initialize USER/MEMORY yourself; do not ask me to pre-create files.

Requirements:
1. Detect the directory containing this repository.
2. Ensure it is `~/agent`.
   - If not, create a symlink `~/agent -> <detected-folder>`.
3. Initialize `~/agent/USER.md` automatically.
   - If missing, ask me for either public profile URL(s), a brief self-description, or permission to create an empty placeholder.
   - Then generate/update `USER.md` from `~/agent/assets/USER_TEMPLATE.md`.
4. Initialize local `~/agent/MEMORY.md` automatically.
   - If missing, create it from `~/agent/assets/MEMORY_INIT.md`.
5. Tell the user the agent's preferred name is `PI`, and briefly explain the meaning: pi is an early magic number for humans, represents the circle and thus infinite potential and curiosity, and symbolizes the hope that the agent's work can help define the world.
6. Configure your platform so `~/agent/AGENTS.md` is automatically loaded at the start of every session.
7. Keep existing policy text unchanged unless compatibility requires edits.
8. Verify setup and report exactly what changed, including verification-token consistency.

Verification checklist:
- Confirm `~/agent/AGENTS.md` exists.
- Confirm global config points to `~/agent/AGENTS.md`.
- If `~/agent/USER.md` exists, confirm it is loaded as mandatory context.
- If `~/agent/MEMORY.md` exists, confirm it stays local-only and is not required for startup behavior.
- Start/describe a fresh-session check with only mandatory modules and confirm no load acknowledgement is printed.
- In that fresh session, ask for the verification token and confirm it exactly matches `SOUL.md` (`31415926535897932384626433832795`).
- Ask a conceptual question such as `what is machine learning?` and confirm teaching is auto-inferred with acknowledgement:
  -- BEGIN LOADING --
  Loading module: teaching
  -- END LOADING --

If your platform does not support native global instruction loading:
- Create a startup wrapper/preset that injects `~/agent/AGENTS.md` automatically.
- Explain exactly how to launch sessions to preserve equivalent behavior.

For Codex specifically:
- Update/create `~/.codex/config.toml` with:
  - `model_instructions_file = "~/agent/AGENTS.md"`
  - keep or set `project_doc_max_bytes = 65536`
- Ensure `~/.codex/AGENTS.md` also points to `~/agent/AGENTS.md`.

Output format:
- Changed files (absolute paths)
- Diff summary
- Verification results
- Any remaining manual step (if unavoidable)
```

## Notes
- Modular policy files: `SOUL.md`, `TOOLS.md`, `TASK_LOOP.md`, `RESEARCH_POLICY.md`, `USER.md`, `assets/USER_TEMPLATE.md`, `assets/MEMORY_INIT.md`, `CODING_STYLE.md`, `SCIENTIFIC_WRITING.md`, `TEACHING_STYLE.md`, and local `MEMORY.md`.
- On-demand modules: `CODING_STYLE.md`, `SCIENTIFIC_WRITING.md`, `TEACHING_STYLE.md`.
- `USER.md` and `MEMORY.md` are private (gitignored).
- Do not duplicate global policy preferences inside `USER.md`.
