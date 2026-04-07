# USER_TEMPLATE

Purpose:
- Generate or update `USER.md` from this template.
- Ask the user for profile input before writing `USER.md`.

## Required Input
Provide one:
- Option A: public profile URL(s) (personal site, CV, scholar page, etc.).
- Option B: brief self-description (5-15 bullets).

If essentials are missing, ask short follow-ups for:
- primary/secondary domains
- strongest topics
- technical stack and tool familiarity
- active learning targets
- stable working preferences that materially affect collaboration (for example GitHub review disclosure or tone preferences)

## Output Format (`USER.md`)
Generate concise sections:
1. Knowledge Domains
2. Technical Ability
3. Strengths
4. Learning Targets
5. Working Preferences (only when the user has stable collaboration preferences worth preserving)

## Constraints
- Keep `USER.md` focused on user knowledge, ability, and stable working preferences.
- Do not duplicate global policy already covered elsewhere unless the user explicitly wants that preference here.
- Do not infer `want to learn X` from `no experience in X` unless the user explicitly says so.
- Do not add personal identifiers unless explicitly provided.
- For GitHub review disclosure preferences, prefer wording that reveals agent authorship without exposing personal information.
- If information is missing, leave a short `fill manually` placeholder.
