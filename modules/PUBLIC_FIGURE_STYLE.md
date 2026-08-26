# PUBLIC_FIGURE_STYLE

## Scope
- A public figure is any plot, diagram, illustration, or visual explanation intended for an external audience or public artifact, including papers, preprints, posters, talks, blogs, websites, documentation, READMEs, reports, repositories, and release assets.
- The intended destination determines the standard: inspect the figure in the actual paper column, responsive article, README renderer, slide, or other delivery surface rather than treating the source file or an enlarged preview as the deliverable.
- When a figure accompanies prose, keep its message, caption, alt text, and nearby explanation consistent. The prose module controls the surrounding writing; this module controls the visual artifact, integration, rendering, and completion gate.

## Requests
- **Refine an existing public figure**
  - **Done when:** at every intended final-use size, the render conveys the intended comparison or mechanism, preserves the underlying meaning, matches the surrounding visual language, and passes the completion gate.
  - **Workflow:** message, audience, delivery surface, and final format → inspect source, render, and surrounding context → highest-impact failure → smallest coherent change → render on the target surface and reinspect.
- **Create or integrate a code-generated public figure**
  - **Done when:** the encoding supports the intended claim, the figure matches nearby public visuals, labels and explanatory context are correct, and every required format passes the completion gate. New or substantively changed generation code must reproduce the final artifact from declared project inputs and workflow; add no reproducibility machinery for visual- or integration-only edits.
  - **Workflow:** claim and data contract → audience and delivery surfaces → clearest encoding → implement with existing project conventions → render at final sizes → inspect and refine → integrate and validate → final artifact and quick preview when useful.
- **Create or revise a diagram, vector figure, or TikZ figure**
  - **Done when:** structure, notation, spacing, and visual language fit the publication context, and the artifact remains understandable at final-use size rather than merely compiling or opening.
  - **Workflow:** message and publication context → reuse existing styles and semantic conventions → construct semantic objects and relationships → render on each target surface → inspect → refine.
  - Use TikZ mainly for conceptual scientific figures. For a new standalone manuscript figure, default to a complete `figure` block with caption and label; preserve an existing figure's integration contract unless restructuring is requested. For web, documentation, or repository delivery, use a compatible public format and supply accessible embedding context.

Use the applicable workflow plus the shared rules below.

## Visual Method
- Match the established publication template and nearby public figures unless a new style is requested. Favor compact footprint, limited excess whitespace, balanced spacing, restrained typography, and clear hierarchy.
- Preserve the data contract and scientific interpretation. Do not silently change values, scales, normalization, uncertainty, comparison baselines, or data-to-mark mapping; expose necessary transformations in the figure or caption.
- Keep legends, annotations, ticks, and labels concise and attributable; remove decoration that competes with the scientific message.
- Give each figure a distinct explanatory job. Remove or redesign a figure that merely repeats nearby prose or another visual unless the repetition enables a deliberate comparison.
- Keep layouts clean; avoid negative `\vspace` and aggressive squeezing unless explicitly requested.
- Reuse project commands, aliases, and layout conventions before introducing new ones. Search only the current project for reusable styles unless given an external reference. Prefer semantic aliases over raw inline styling and semantic names without forced personal prefixes; replace scattered hardcoded geometry with named coordinates, macros, or nodes when clearer.
- If no needed style or macro exists, first seek a compatible local convention. Before changing a shared header, macro, or style, present at most two options, explain likely blast radius, and obtain approval. A local-only addition may proceed without confirmation when it follows the visual language and does not materially change the result.
- Treat a user-provided manual drawing or adjustment as the primary visual source of truth; bias toward faithful cleanup and integration unless replacement is explicitly requested.
- Ask for a style choice only when it materially changes the result and cannot be inferred. Otherwise use the closest convention and proceed; present at most two alternatives when needed.

## Public Integration and Release
- Establish the target surfaces, containers, and final physical or pixel sizes before styling. Paper-column width, responsive article width, README rendering, slide projection, and downloadable release assets impose different constraints.
- For responsive surfaces, inspect representative narrow, intermediate, and wide container widths. Wide figures must reflow, remain legible while scaling, or use an intentional scroll treatment with a visible cue, keyboard access, and both endpoints checked; never create page-wide overflow.
- Provide explanatory context appropriate to the surface: a caption or nearby prose plus meaningful alt text for embedded web or documentation figures. A standalone SVG should include a concise `<title>` and `<desc>` and appropriate image semantics when the delivery path preserves them.
- Keep text readable at final size and important distinctions available without color alone. Verify contrast, color-vision deficiency, grayscale when relevant, and both screen and print behavior for destinations that need them.
- Deliver the required final-use format and retain editable source when the project expects it. Prefer repository-relative stable paths, verify every embedding or download link, and exclude obsolete variants, temporary renders, debug layers, and local absolute paths from public release.
- Before release, check figures and metadata for secrets, sensitive or unintended personal information, and unapproved third-party material. Verify reuse rights and required attribution for external assets.

## Render and Completion Gate
- After every appearance-sensitive change, inspect the render—not only source or compilation—for overlap, clipping, crowding, alignment, ambiguous annotations, and readability at final-use size.
- Iterate through the fastest reliable draft path, then use the required final path before delivery. Use multiple LaTeX passes only when references or layout require them.
- Prefer vector plots and diagrams when the toolchain and delivery surface permit. For raster content, verify sufficient resolution at final physical or pixel size; do not infer quality from an enlarged preview.
- The figure remains incomplete with overlap or clipping, unreadable labels, inconsistent fonts or line styles, unbalanced spacing or alignment, incorrect explanatory context, inaccessible encoding, broken or misleading responsive behavior, invalid integration, or mismatch with nearby public figures. Fix the highest-impact failure next; if blocked, report the exact failure, render evidence, and one primary plan.
- After two consecutive misses of the intended visual structure, explicitly change strategy instead of continuing local tweaks.
- Iterate until the gate passes or 10 render-refine passes. At the cap, give exactly one primary fix plan with estimated effort and await approval before continuing.

### PIRA SVG check
- For a public SVG containing semantic `<text>`, run `pira_svg_check FIGURE.svg` after the final render and before release. Use `--json` for machine-readable output and repeat `--font-dir DIR` when the final fonts are not otherwise available to the renderer.
- Treat findings as conservative review warnings, not rejection criteria. Inspect every cited label in the final-use render and fix confirmed low contrast, clipping or masking, text overlap, or stroke intrusion. An opaque background behind text is acceptable when the text remains readily legible and foreground content does not obstruct it.
- A clear result does not replace the visual completion gate: path-converted text is not discoverable as text, and complex filters, `foreignObject`, text paths, unusual blending, browser-specific layout, or transparent-canvas assumptions can still require manual review. Tool analysis errors must be resolved or reported rather than treated as a clear result.

## Color
- Reuse the publication's palette when present. Otherwise choose by purpose, keep mappings consistent, and verify contrast, color-vision deficiency, and grayscale at final size. Add a non-color cue for important distinctions.
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
