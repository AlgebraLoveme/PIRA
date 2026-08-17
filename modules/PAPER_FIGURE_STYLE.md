# PAPER_FIGURE_STYLE

## Requests
- **Refine an existing paper figure**
  - **Done when:** at final-use size, the render conveys the intended comparison or mechanism, preserves scientific meaning, matches the paper's visual language, and passes the completion gate.
  - **Workflow:** message and final format → inspect source and render → highest-impact failure → smallest coherent change → render and reinspect.
- **Create or integrate a code-generated paper figure**
  - **Done when:** the figure matches nearby figures, is correctly labeled, and has the required final format. If integration is in scope, caption, label, and references must be correct. New or substantively changed generation code must reproduce from declared project inputs and workflow; add no reproducibility machinery for visual- or integration-only edits.
  - **Workflow:** claim and data contract → clearest encoding → implement with existing project conventions → render at final size → inspect and refine → final artifact and quick preview when useful.
- **Create or revise a TikZ figure**
  - **Done when:** structure, notation, spacing, and figure language fit the manuscript, not merely compile.
  - **Workflow:** message and manuscript context → reuse existing styles and macros → construct semantic nodes and relationships → compile → inspect → refine.
  - Use TikZ mainly for conceptual scientific figures. For a new standalone manuscript figure, default to a complete `figure` block with caption and label; preserve an existing figure's integration contract unless restructuring is requested.

Use the applicable workflow plus the shared rules below.

## Visual Method
- Match the established paper template and nearby figures unless a new style is requested. Favor compact footprint, limited excess whitespace, balanced spacing, restrained typography, and clear hierarchy.
- Preserve the data contract and scientific interpretation. Do not silently change values, scales, normalization, uncertainty, comparison baselines, or data-to-mark mapping; expose necessary transformations in the figure or caption.
- Keep legends, annotations, ticks, and labels concise and attributable; remove decoration that competes with the scientific message.
- Keep layouts clean; avoid negative `\vspace` and aggressive squeezing unless explicitly requested.
- Reuse project commands, aliases, and layout conventions before introducing new ones. Search only the current project for reusable styles unless given an external reference. Prefer semantic aliases over raw inline styling and semantic names without forced personal prefixes; replace scattered hardcoded geometry with named coordinates, macros, or nodes when clearer.
- If no needed style or macro exists, first seek a compatible local convention. Before changing a shared header, macro, or style, present at most two options, explain likely blast radius, and obtain approval. A local-only addition may proceed without confirmation when it follows the visual language and does not materially change the result.
- Treat a user-provided manual drawing or adjustment as the primary visual source of truth; bias toward faithful cleanup and integration unless replacement is explicitly requested.
- Ask for a style choice only when it materially changes the result and cannot be inferred. Otherwise use the closest convention and proceed; present at most two alternatives when needed.

## Render and Completion Gate
- After every appearance-sensitive change, inspect the render—not only source or compilation—for overlap, clipping, crowding, alignment, ambiguous annotations, and readability at final-use size.
- Iterate through the fastest reliable draft path, then use the required final path before delivery. Use multiple LaTeX passes only when references or layout require them.
- Prefer vector plots and diagrams when the toolchain and venue permit. For raster content, verify sufficient resolution at final physical size; do not infer quality from an enlarged preview.
- The figure remains incomplete with overlap or clipping, unreadable labels, inconsistent fonts or line styles, unbalanced spacing or alignment, incorrect caption or label, or mismatch with nearby figures. Fix the highest-impact failure next; if blocked, report the exact failure, render evidence, and one primary plan.
- After two consecutive misses of the intended visual structure, explicitly change strategy instead of continuing local tweaks.
- Iterate until the gate passes or 10 render-refine passes. At the cap, give exactly one primary fix plan with estimated effort and await approval before continuing.

## Color
- Reuse the paper's palette when present. Otherwise choose by purpose, keep mappings consistent, and verify contrast, color-vision deficiency, and grayscale at final size. Add a non-color cue for important distinctions.
- Use these colors for categorical marks and concept or workflow diagrams. Ordered data require a perceptually uniform sequential, diverging, or cyclic map matched to meaning.

- Contrastive: `#D95F68`, `#3E8FC4`, `#E3A72F`, `#49A781`, `#8A70B5`, `#7A858C`.
- Balanced categorical: `#5B8EC8`, `#E98778`, `#70BDD6`, `#82B982`, `#B184C1`, `#D9AF4B`.
- Similar but distinguishable, dark to pale. Use dark values for lines or points and pale values for fills or backgrounds.
  - `purple`: `#59449B`, `#C5B9DD`, `#E5E0EF`.
  - `blue`: `#2F5DA8`, `#8FC3DC`, `#CEE5EF`.
  - `teal`: `#3B8587`, `#A6D3D0`, `#DCEEEB`.
  - `green`: `#347A45`, `#B4D7AF`, `#E1EFDE`.
  - `amber`: `#B9822F`, `#E9C67F`, `#F5E5BF`.
  - `coral`: `#B9473E`, `#F3BAAA`, `#F9E0D8`.
  - `rose`: `#B65A7B`, `#EAB9C9`, `#F6DFE7`.
  - `slate`: `#5B7088`, `#B5C1CC`, `#E4E9ED`.
- Paired groups use two hue families at matching lightness positions. Common choices: `cool/warm`—blue (`#2F5DA8`, `#8FC3DC`) with coral (`#B9473E`, `#F3BAAA`); `green/purple`—green (`#347A45`, `#B4D7AF`) with purple (`#59449B`, `#C5B9DD`).
