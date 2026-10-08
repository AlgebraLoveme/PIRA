# PIRA SVG Check

`pira_svg_check` is a conservative, warning-only Rust linter for semantic text in static SVG figures. It checks:

- low local contrast against the fully composited background;
- clipping or masking that removes glyph pixels;
- visible stroked paths that intersect glyphs or traverse a protected text block;
- direct overlap between rendered text glyphs.

Warnings never make the CLI fail. Invalid SVG, unsafe external resources, renderer failures, unsupported scene analysis, missing isolated fonts, and invalid options return exit code `2`.

The binary embeds the pure-Rust `resvg` renderer and has no Python, browser, or system SVG-renderer dependency. System fonts are loaded by default; repeatable `--font-dir DIR` arguments add fonts. For an isolated inventory, use `--isolated-fonts --font-dir DIR` (also `Config::isolated_fonts` in the library). This mode never loads system fonts. Pin the supplied font files and name their families explicitly in the SVG. Generic families use fontdb's fixed defaults, not host aliases; missing family selection or unresolved final shaped glyphs is an actionable analysis error, not a silent system fallback. The inventory may supply glyph fallback fonts. Final renderer glyph output is checked as well as selection callbacks, including text in dependency subtrees; a candidate fallback font is not proof of successful reshaping. An inaccessible directory is an error. Isolation controls fonts, not cross-platform floating-point rasterization.

## Usage

Install or refresh the released binary with PIRA's setup tool:

```sh
python3 assets/scripts/setup_pira_tools.py --tool pira_svg_check
```

Then run `pira_svg_check figure.svg`. The setup tool selects the current platform,
verifies the release metadata, size, SHA-256 checksum, and reported version, and
installs into the per-user PIRA tools directory.

From the repository:

```sh
cargo run --manifest-path tools/Cargo.toml -p pira_svg_check -- figure.svg
cargo run --manifest-path tools/Cargo.toml -p pira_svg_check -- --json figure.svg
```

## Interpretation and limits

Local fragment references and applied scene clips/masks/filters are resolved through usvg before analysis rewrites, including nested/repeated `<use>` and scene-backed mask artwork. Original object-bounding-box effect geometry is frozen before hiding sibling geometry, regardless of whether any `href` occurs in the input. Effect-free no-reference inputs retain source analysis (including hidden-text diagnostics). Each resolved text/paint instance is analyzed independently. Renderer serialization must reproduce the original raster at the requested scale exactly; otherwise the tool fails with `unsupported scene analysis` instead of issuing misleading findings. This conservative gate may reject serialization precision/layout limitations; inline or simplify that artwork. Attribution uses retained renderer IDs when available and otherwise resolved element ordinals (`<text@n…>`), not source line numbers. Renderer-elided invisible content in canonicalized scenes is not an analyzable instance.

Glyph-core coverage is rendered separately with scene opacity and text fill-opacity neutralized, so a faint tspan cannot disappear behind the element's opaque-span alpha threshold. Actual composited colors still determine contrast. The original painted-alpha probes still determine clipping loss and wholly unrendered text; visibility/display, fills, clips, masks and dependency artwork remain in the coverage probe. This is renderer-supported text geometry, not browser layout. The painted probe is dropped before coverage allocation, preserving the retained-plus-next-mask budget.

`text-clipped` requires measured loss of isolated glyph alpha, across attributes, inline styles and stylesheet declarations alike. A full-coverage or overridden clip does not warn. The unclipped probe removes scene clipping/masking while leaving dependency artwork inside definitions intact.

The guard is deliberately conservative. It treats the composite of everything beneath a text element as its background, so explicit label backgrounds are optional. A fully opaque backing naturally hides plot lines and prevents an intrusion warning.

The protected block is the rendered glyph bound plus proportional padding. A visible stroke can warn when it crosses this block even if topmost opaque glyphs hide parts of the stroke. Contrast and visual clutter are perceptual proxies, not proofs of readability.

Input reading is capped (8,000,000 bytes by default), including for finite stream inputs. Before recursive renderer parsing, an iterative scan enforces at most 64 levels of XML elements (including empty leaves) and 50,000 elements per parsed document. For embedded SVG data, the maximum XML depths of simultaneously active documents are added conservatively against the same 64-level ceiling; at most four embedded SVG layers are allowed. Both `image/svg+xml` and SVG sniffed from `text/plain` are checked before parsing, including image/feImage data. Excess nesting returns exit 2, not a renderer abort. Compressed SVG is unsupported; embedded DTDs are rejected. These conservative limits are not a guarantee for arbitrarily small library-caller thread stacks. Each render is limited to 16,000,000 pixels. Retained RGBA text masks **plus the next text-mask allocation** have a fixed 256 MiB budget; exceeding it is an analysis error (exit 2). The check precedes allocation, so a label can be refused even if its mask would later be discarded as unrendered. This is **not a total-process/RSS limit**: other render buffers, fonts and parser/renderer allocations are outside this budget. There is no CLI or configuration override.

CSS resource validation is applied to stylesheets, inline styles and resource-bearing presentation attributes, not metadata or ordinary text. Local fragments (including quoted/encoded forms) and data references are allowed; external references and CSS imports are rejected. The renderer cannot load filesystem images: unresolved fragments and malformed data URLs never fall back to cwd-relative filenames. Supported scene fragments and embedded images retain renderer semantics. The library's `block_padding_fraction` must be finite and nonnegative, with no upper bound.

Only semantic `<text>` elements are discoverable. Text converted to paths cannot be distinguished reliably from ordinary figure geometry. Complex filters, `foreignObject`, text on paths, unusual blending, and browser-specific layout may require human review. A white canvas is assumed when the SVG itself is transparent.

## Tests

```sh
cargo test --manifest-path tools/Cargo.toml -p pira_svg_check
```


Rendering tests require installed fonts. Mask-budget tests explicitly select a
verified available family in disposable fixture copies; they do not depend on
Times New Roman or a host generic-family alias. With no usable font they fail
with a prerequisite diagnostic (including loaded face count and default mapping),
not a skipped or weakened byte-bound assertion. Production default mode is
unchanged: if no font can render a label it reports `text-not-rendered`; there is
no glyph mask to retain. Isolated mode retains its stricter font errors.

For a minimal Debian/Ubuntu native CI runner, provision `fontconfig` and
`fonts-dejavu-core` **in the CI environment**, then, from the repository root:

```sh
# Provisioning belongs to the CI owner, not the tool or its test process.
sudo apt-get update
sudo apt-get install -y --no-install-recommends fontconfig fonts-dejavu-core
FONTCONFIG_FILE="$PWD/tools/crates/pira_svg_check/tests/fixtures/linux-dejavu-fonts.conf" \
  cargo test --offline --locked --manifest-path tools/Cargo.toml -p pira_svg_check
```

The test-only fontconfig file pins Linux discovery and generic aliases to that
installed inventory without changing user font configuration or production
policy. For another distro, supply equivalent font files/configuration explicitly.
The complex-script fallback regression is macOS-only and additionally requires
Georgia and Arial Unicode supplemental fonts.
