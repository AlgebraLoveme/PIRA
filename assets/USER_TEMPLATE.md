# USER_TEMPLATE

Purpose
- Use this template to generate or update `USER.md`.
- The agent must first ask the user for profile input, then write `USER.md`.

## Required Input (ask the user)
Provide one:
- Option A: public profile URL(s) (personal site, CV, scholar page, etc.).
- Option B: brief self-description (5-15 bullets).

If needed, ask short follow-ups for missing essentials:
- primary/secondary domains
- strongest topics
- technical stack and tool familiarity
- active learning targets

## Output Format (`USER.md`)
Generate concise sections:
1. Knowledge Domains
2. Technical Ability
3. Strengths
4. Learning Targets

## Constraints
- Keep only user knowledge/ability context.
- Do not encode global policy preferences already covered by other modules.
- Do not infer "want to learn X" from "no experience in X" unless explicitly stated.
- Do not add personal identifiers unless explicitly provided.
- If information is missing, leave a short "fill manually" placeholder.
