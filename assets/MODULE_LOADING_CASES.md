# Module Loading Cases

Use these fresh-session checks to verify routing and load acknowledgement behavior. The prompts suppress task answers so the test isolates module loading.

## Expected acknowledgement format
When one or more optional modules are loaded, print:

```text
-- Loading --
module: <comma-separated-optional-module-list>
-- END --
```

When only mandatory files are loaded, print nothing.

## Cases
1. Conceptual learning question
   - Prompt: `what is machine learning? Do not answer the question itself. If any optional modules are loaded, print only the standard load acknowledgement and nothing else.`
   - Expected modules: `learning, research`

2. Coding task
   - Prompt: `debug this Python function. Do not debug it or explain it. If any optional modules are loaded, print only the standard load acknowledgement and nothing else.`
   - Expected modules: `coding, research`

3. Writing task
   - Prompt: `polish this LaTeX paragraph. Do not rewrite it. If any optional modules are loaded, print only the standard load acknowledgement and nothing else.`
   - Expected modules: `writing, research`

4. Guidance request
   - Prompt: `I feel stuck about my career. Do not give advice. If any optional modules are loaded, print only the standard load acknowledgement and nothing else.`
   - Expected modules: `guidance`

5. Maintenance request
   - Prompt: `update the config. Do not make changes or propose steps. If any optional modules are loaded, print only the standard load acknowledgement and nothing else.`
   - Expected modules: `maintenance`

## Notes
- `coding`, `writing`, and `learning` are research-level by default, so they should also load `research` unless the user clearly wants a casual or non-research interaction.
- `guidance` and `maintenance` do not imply `research` by default.
- The acknowledgement should list only optional modules that were actually loaded.
- The prompts intentionally suppress normal task answers, so any visible output should reflect loading behavior rather than content generation.
