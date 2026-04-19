# Module Loading Cases

Use these fresh-session prompts to verify optional-module routing.

## Expected output
For each case, the prompt should make the agent print only the optional instruction file paths it intends to read, one per line, and then stop.

## Cases
1. Conceptual learning question
   - Prompt: `what is machine learning? Before answering, print only the optional instruction file paths you intend to read, one per line, and then stop without answering.`
   - Expected modules: `learning`
   - Expected files:
     - `~/agent/modules/LEARNING_STYLE.md`

2. Coding task
   - Prompt: `debug this Python function. Before debugging or explaining it, print only the optional instruction file paths you intend to read, one per line, and then stop without debugging or explaining anything.`
   - Expected modules: `coding, research`
   - Expected files:
     - `~/agent/modules/CODING_STYLE.md`
     - `~/agent/modules/RESEARCH_POLICY.md`

3. Writing task
   - Prompt: `polish this LaTeX paragraph. Before rewriting it, print only the optional instruction file paths you intend to read, one per line, and then stop without rewriting anything.`
   - Expected modules: `writing, research`
   - Expected files:
     - `~/agent/modules/SCIENTIFIC_WRITING.md`
     - `~/agent/modules/RESEARCH_POLICY.md`

4. Guidance request
   - Prompt: `I feel stuck about my career. Before giving advice, print only the optional instruction file paths you intend to read, one per line, and then stop without giving advice.`
   - Expected modules: `guidance`
   - Expected files:
     - `~/agent/modules/GUIDANCE.md`

5. Maintenance request
   - Prompt: `update the PIRA config. Before making changes or proposing steps, print only the optional instruction file paths you intend to read, one per line, and then stop without making changes or proposing steps.`
   - Expected modules: `maintenance`
   - Expected files:
     - `~/agent/modules/MAINTENANCE.md`

6. TikZ figure task
   - Prompt: `adjust this TikZ figure layout. Before editing it, print only the optional instruction file paths you intend to read, one per line, and then stop without editing anything.`
   - Expected modules: `writing, research`
   - Expected files:
     - `~/agent/modules/SCIENTIFIC_WRITING.md`
     - `~/agent/modules/RESEARCH_POLICY.md`

7. General plotting code task
   - Prompt: `edit this Python plotting script to change the aggregation logic. Before editing it, print only the optional instruction file paths you intend to read, one per line, and then stop without editing anything.`
   - Expected modules: `coding, research`
   - Expected files:
     - `~/agent/modules/CODING_STYLE.md`
     - `~/agent/modules/RESEARCH_POLICY.md`

8. Paper figure styling task
   - Prompt: `make this matplotlib figure match the paper's visual style and layout. Before restyling it, print only the optional instruction file paths you intend to read, one per line, and then stop without restyling anything.`
   - Expected modules: `coding, writing, research`
   - Expected files:
     - `~/agent/modules/CODING_STYLE.md`
     - `~/agent/modules/SCIENTIFIC_WRITING.md`
     - `~/agent/modules/RESEARCH_POLICY.md`

## Notes
- `coding` and `writing` are research-level by default, so they should also load `research`.
- `learning` does not always imply `research`; add `research` when the task needs factual analysis, evidence-based reporting, online verification, or broader research-style synthesis.
- `guidance` and `maintenance` do not imply `research` by default.
- Each prompt is designed to elicit only the optional instruction file paths the agent intends to read.
