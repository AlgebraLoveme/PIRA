# personal_agent setup

Repository
- https://github.com/AlgebraLoveme/personal_agent

Goal
- Configure your coding model to auto-load this policy globally.
- Source of truth: `~/agent/AGENTS.md` and referenced modules.
- Recommended clone location: `~/agent`.

## Quick start
1. Clone the repo to `~/agent` (recommended).
   - `git clone https://github.com/AlgebraLoveme/personal_agent.git ~/agent`
2. Initialize `~/agent/USER.md`:
   - Option A: copy-paste your own `USER.md`.
   - Option B: ask your agent to request your profile (URL or brief description) and auto-generate `USER.md`.
   - Option C: create `USER.md` later (initialize empty now).
   - Follow `~/agent/assets/USER_TEMPLATE.md`.
3. Initialize local memory (default):
   - If `~/agent/MEMORY.md` does not exist, create it from `~/agent/assets/MEMORY_INIT.md` (pre-filled default memory).
4. Open your coding model tool (Codex / Claude / others).
5. Paste the prompt below and let the model set up + verify.

## Prompt to paste into your model
```
I cloned a repo that contains an agent policy folder. Please set up a global agent pipeline for me.

Requirements:
1. Detect the directory containing this repository.
2. Ensure the directory is `~/agent`.
   - If not, create a symlink `~/agent -> <detected-folder>`.
3. Initialize `~/agent/USER.md`:
   - Ask me for either:
     a) public profile URL(s), or
     b) a brief self-description, or
     c) user creates USER.md later.
   - Then generate/update `USER.md` (for option c, create an empty file).
   - Follow `~/agent/assets/USER_TEMPLATE.md`.
4. Initialize local `~/agent/MEMORY.md` if missing (copy from `~/agent/assets/MEMORY_INIT.md`, which contains the default initial memory).
5. Configure your platform so `~/agent/AGENTS.md` is automatically loaded at the start of every session.
6. Keep existing policy text unchanged unless needed for compatibility.
7. Verify setup and report exactly what changed, including verification-token consistency.

Verification checklist:
- Confirm `~/agent/AGENTS.md` exists.
- Confirm global config points to `~/agent/AGENTS.md`.
- If `~/agent/USER.md` exists, confirm it is loaded as mandatory context.
- If `~/agent/MEMORY.md` exists, confirm it remains local-only and is not required for startup behavior.
- Start/describe a fresh-session check with only mandatory modules and confirm no load acknowledgement is printed.
- In that fresh session, ask for the verification token and confirm it exactly matches `SOUL.md` (`31415926535897932384626433832795`).
- Ask a conceptual question (e.g., "what is machine learning?") and confirm teaching is auto-inferred with acknowledgement:
  -- BEGIN LOADING --
  Loading module: teaching
  -- END LOADING --

If your platform does not support native global instruction loading:
- Create a startup wrapper/preset that injects `~/agent/AGENTS.md` automatically.
- Explain exactly how to launch sessions to keep equivalent behavior.

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
- Modular policy files:
  `SOUL.md`, `TOOLS.md`, `TASK_LOOP.md`, `RESEARCH_POLICY.md`,
  `USER.md`, `assets/USER_TEMPLATE.md`, `assets/MEMORY_INIT.md`, `CODING_STYLE.md`, `SCIENTIFIC_WRITING.md`, `TEACHING_STYLE.md`, and local `MEMORY.md`.
- `CODING_STYLE.md`, `SCIENTIFIC_WRITING.md`, and `TEACHING_STYLE.md` are on-demand modules.
- `USER.md` and `MEMORY.md` are private (gitignored).
- Do not duplicate global policy preferences in `USER.md`.
