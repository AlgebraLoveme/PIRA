# PIRA SVG Check

`pira_svg_check` is a conservative, warning-only Rust linter for semantic text in static SVG figures. It checks:

- low local contrast against the fully composited background;
- clipping or masking that removes glyph pixels;
- visible stroked paths that intersect glyphs or traverse a protected text block;
- direct overlap between rendered text glyphs.

Warnings never make the CLI fail. Invalid SVG, unsafe external resources, renderer failures, and invalid options return exit code `2`.

The binary embeds the pure-Rust `resvg` renderer and has no Python, browser, or system SVG-renderer dependency. System fonts are loaded by default; use repeatable `--font-dir` arguments and a pinned font set when cross-machine pixel reproducibility matters.

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

The guard is deliberately conservative. It treats the composite of everything beneath a text element as its background, so explicit label backgrounds are optional. A fully opaque backing naturally hides plot lines and prevents an intrusion warning.

The protected block is the rendered glyph bound plus proportional padding. A visible stroke can warn when it crosses this block even if topmost opaque glyphs hide parts of the stroke. Contrast and visual clutter are perceptual proxies, not proofs of readability.

Only semantic `<text>` elements are discoverable. Text converted to paths cannot be distinguished reliably from ordinary figure geometry. Complex filters, `foreignObject`, text on paths, unusual blending, and browser-specific layout may require human review. A white canvas is assumed when the SVG itself is transparent.

## Tests

```sh
cargo test --manifest-path tools/Cargo.toml -p pira_svg_check
```
