# USER_TEMPLATE

Purpose:
- Use this template to generate or update `USER.md`.
- Ask the user for profile input before writing `USER.md`.

## Required Input (ask the user)
Provide one:
- Option A: public profile URL(s) (personal site, CV, scholar page, etc.).
- Option B: brief self-description (5-15 bullets).

If essentials are missing, ask short follow-ups for:
- primary/secondary domains
- strongest topics
- technical stack and tool familiarity
- active learning targets
- any user-specific working preferences that materially affect collaboration (for example, GitHub review disclosure or tone preferences)

## Output Format (`USER.md`)
Generate concise sections:
1. Knowledge Domains
2. Technical Ability
3. Strengths
4. Learning Targets
5. Working Preferences (only when the user has stable collaboration preferences worth preserving, such as GitHub review disclosure or tone preferences)

## Constraints
- Keep `USER.md` focused on user knowledge, ability, and stable working preferences.
- Do not duplicate global policy preferences already covered elsewhere unless the user specifically wants the preference to live in `USER.md`.
- Do not infer `want to learn X` from `no experience in X` unless the user explicitly says so.
- Do not add personal identifiers unless explicitly provided.
- For GitHub review disclosure preferences, prefer wording that reveals agent authorship without exposing the user's personal information.
- If information is missing, leave a short `fill manually` placeholder.
