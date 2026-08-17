# PAPER_FIGURE_STYLE

## Requests
- **Refine an existing paper figure**
  - **Done when:** the rendered figure communicates the intended comparison or mechanism at its final-use size, preserves the scientific meaning, matches the paper's visual language, and passes the completion gate below.
  - **Workflow:** intended message and final format → inspect source and current render → identify the highest-impact visual failure → make the smallest coherent change → render and inspect again.
- **Create or integrate a code-generated paper figure**
  - **Done when:** the figure is visually consistent with nearby figures, correctly labeled, and delivered in the required final-use format. When manuscript integration is in scope, its caption, label, and references must also be correct. New or substantively changed generation code must be reproducible from the project's declared inputs and workflow; do not add reproducibility machinery for a visual- or integration-only edit.
  - **Workflow:** claim and data contract → choose the clearest visual encoding → implement using existing project conventions → render at final size → inspect and refine → export final artifact and a quick preview when useful.
- **Create or revise a TikZ figure**
  - **Done when:** structure, notation, spacing, and figure language fit the target manuscript—not merely when the code compiles.
  - **Workflow:** message and manuscript context → reuse existing styles/macros → construct semantic nodes and relationships → compile → inspect → refine.
  - Use TikZ mainly for conceptual scientific figures. Default to a complete `figure` block with caption and label when creating a standalone manuscript figure. For an existing figure, preserve its integration contract unless the user requests restructuring.

Use the applicable request workflow plus the shared rules below.

## Visual Method
- Match the established paper template and nearby figures unless a new style is requested. Favor a compact footprint, limited excess whitespace, balanced spacing, restrained typography, and a clear information hierarchy.
- Preserve the data contract and scientific interpretation. Do not silently change values, scales, normalization, uncertainty, comparison baselines, or the mapping from data to visual marks; make necessary transformations explicit in the figure or caption.
- Keep legends, annotations, ticks, and labels concise and attributable. Eliminate decorative elements that compete with the scientific message.
- Keep layouts clean; avoid negative `\vspace` and aggressive squeezing unless explicitly requested.
- Reuse existing commands, style aliases, and layout conventions before introducing new ones. Search only the current project for reusable styles unless the user provides an external reference. Prefer semantic style aliases to raw inline styling. Give new reusable styles semantic names without forcing personal prefixes; avoid scattered hardcoded geometry when named coordinates, macros, or nodes would clarify structure.
- If a needed style or macro is absent, first look for a compatible local convention. Before editing a shared header, macro, or style definition, present at most two options, explain the likely blast radius, and obtain approval. A local-only addition may proceed without confirmation when it follows the established visual language and does not materially change the result.
- A user-provided manual drawing or adjustment is the primary visual source of truth. Bias toward faithful cleanup and integration unless replacement is explicitly requested.
- Ask for a style choice only when it materially changes the result and cannot be inferred. Otherwise use the closest existing convention and proceed; if alternatives are needed, present at most two.

## Render and Completion Gate
- After every appearance-sensitive change, inspect the render—not only source or compilation—for overlap, clipping, crowding, alignment, annotation ambiguity, and readability at final-use size.
- Compile or render with the fastest reliable draft path during iteration, then use the required final path before delivery. Use multiple LaTeX passes only when references or layout require them.
- Prefer vector output for plots and diagrams when the project toolchain and venue permit it. For raster content, verify sufficient resolution at the final physical size; do not infer output quality from an enlarged preview.
- A figure is incomplete while any acceptance item fails, including overlap or clipping, unreadable labels, inconsistent fonts or line styles, unbalanced spacing or alignment, an incorrect caption or label, or mismatch with nearby figures. Continue with the highest-impact fix; if progress is blocked, report the exact visual failure, evidence from the render, and one primary fix plan.
- Two consecutive misses of the intended visual structure require an explicit strategy change rather than more local tweaking.
- Iterate until the completion gate passes or 10 render-refine passes have been attempted. At the cap, provide exactly one primary fix plan with estimated effort and wait for approval before continuing.

## Color
- Reuse the paper's palette when present. Otherwise choose by purpose below, keep mappings consistent, and verify contrast, color-vision deficiency, and grayscale at final size. Add a non-color cue for important distinctions.
- Use these colors for categorical marks and concept or workflow diagrams. Ordered data require a perceptually uniform sequential, diverging, or cyclic map matched to the data meaning.

- Contrastive: `#D95F68`, `#3E8FC4`, `#E3A72F`, `#49A781`, `#8A70B5`, `#7A858C`.
- Balanced categorical: `#5B8EC8`, `#E98778`, `#70BDD6`, `#82B982`, `#B184C1`, `#D9AF4B`.
- Similar but distinguishable, dark to pale. Use darker values for lines or points and pale values for fills or backgrounds.
  - `purple`: `#59449B`, `#C5B9DD`, `#E5E0EF`.
  - `blue`: `#2F5DA8`, `#8FC3DC`, `#CEE5EF`.
  - `teal`: `#3B8587`, `#A6D3D0`, `#DCEEEB`.
  - `green`: `#347A45`, `#B4D7AF`, `#E1EFDE`.
  - `amber`: `#B9822F`, `#E9C67F`, `#F5E5BF`.
  - `coral`: `#B9473E`, `#F3BAAA`, `#F9E0D8`.
  - `rose`: `#B65A7B`, `#EAB9C9`, `#F6DFE7`.
  - `slate`: `#5B7088`, `#B5C1CC`, `#E4E9ED`.
- Paired groups: choose two hue families and use matching lightness positions. Common choices are `cool/warm`: blue (`#2F5DA8`, `#8FC3DC`) with coral (`#B9473E`, `#F3BAAA`); and `green/purple`: green (`#347A45`, `#B4D7AF`) with purple (`#59449B`, `#C5B9DD`).
