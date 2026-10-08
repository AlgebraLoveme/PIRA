mod css;
mod style;
mod xml;

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;
use serde::Serialize;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::xml::{AnnotatedSvg, ElementMeta, Variant};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_PIXELS: u64 = 16_000_000;
const MAX_TEXT_MASK_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Config {
    pub scale: f32,
    pub min_contrast: f64,
    pub max_low_contrast_fraction: f64,
    pub crossing_ratio: f64,
    /// Proportional text-block padding; must be finite and nonnegative.
    pub block_padding_fraction: f64,
    pub max_svg_bytes: usize,
    pub font_dirs: Vec<PathBuf>,
    /// Resolve fonts only from font_dirs; missing selection or glyphs is an error.
    pub isolated_fonts: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scale: 2.0,
            min_contrast: 4.5,
            max_low_contrast_fraction: 0.05,
            crossing_ratio: 0.45,
            block_padding_fraction: 0.12,
            max_svg_bytes: 8_000_000,
            font_dirs: Vec::new(),
            isolated_fonts: false,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
pub struct GuardWarning {
    pub code: String,
    pub message: String,
    pub text: Option<String>,
    pub text_element: Option<String>,
    pub interfering_element: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub source: String,
    pub renderer: String,
    pub status: &'static str,
    pub warnings: Vec<GuardWarning>,
}

#[derive(Debug)]
pub struct GuardError(String);

impl fmt::Display for GuardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for GuardError {}

impl From<String> for GuardError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug)]
struct Bounds {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl Bounds {
    fn width(self) -> u32 {
        self.x1 - self.x0
    }

    fn height(self) -> u32 {
        self.y1 - self.y0
    }

    fn expanded(self, pixels: u32, width: u32, height: u32) -> Self {
        Self {
            x0: self.x0.saturating_sub(pixels),
            y0: self.y0.saturating_sub(pixels),
            x1: self.x1.saturating_add(pixels).min(width),
            y1: self.y1.saturating_add(pixels).min(height),
        }
    }

    fn intersects(self, other: Self) -> bool {
        self.x0 < other.x1 && other.x0 < self.x1 && self.y0 < other.y1 && other.y0 < self.y1
    }
}

struct TextRegion {
    meta: ElementMeta,
    mask: Pixmap,
    core_threshold: u8,
    glyph_bounds: Bounds,
    protected_bounds: Bounds,
}

struct Renderer {
    fontdb: Arc<usvg::fontdb::Database>,
    scale: f32,
    isolated_fonts: bool,
}

impl Renderer {
    fn new(config: &Config) -> Self {
        let mut database = usvg::fontdb::Database::new();
        if !config.isolated_fonts {
            database.load_system_fonts();
        }
        for directory in &config.font_dirs {
            database.load_fonts_dir(directory);
        }
        Self {
            fontdb: Arc::new(database),
            scale: config.scale,
            isolated_fonts: config.isolated_fonts,
        }
    }

    fn parse(&self, source: &[u8]) -> Result<usvg::Tree, GuardError> {
        let depth = xml::render_depth(source, 0)?;
        let resource_state = Mutex::new((depth, 0_usize, None));
        let data_resolver = usvg::ImageHrefResolver::default_data_resolver();
        let issues = Mutex::new(Vec::new());
        let select = usvg::FontResolver::default_font_selector();
        let fallback = usvg::FontResolver::default_fallback_selector();
        let resolver = usvg::FontResolver {
            select_font: Box::new(|font, db| {
                let id = select(font, db).filter(|id| {
                    if !self.isolated_fonts {
                        return true;
                    }
                    let face = db.face(*id).unwrap();
                    font.families().iter().any(|family| {
                        use usvg::fontdb::Family;
                        let family = match family {
                            usvg::FontFamily::Named(name) => Family::Name(name),
                            usvg::FontFamily::Serif => Family::Serif,
                            usvg::FontFamily::SansSerif => Family::SansSerif,
                            usvg::FontFamily::Monospace => Family::Monospace,
                            usvg::FontFamily::Cursive => Family::Cursive,
                            usvg::FontFamily::Fantasy => Family::Fantasy,
                        };
                        face.families
                            .iter()
                            .any(|(name, _)| name == db.family_name(&family))
                    })
                });
                if self.isolated_fonts && id.is_none() {
                    issues
                        .lock()
                        .unwrap()
                        .push(format!("missing font family {:?}", font.families()));
                }
                id
            }),
            select_fallback: Box::new(|ch, used, db| {
                let id = fallback(ch, used, db);
                if self.isolated_fonts && id.is_none() {
                    issues
                        .lock()
                        .unwrap()
                        .push(format!("missing glyph U+{:04X}", u32::from(ch)));
                }
                id
            }),
        };
        let options = usvg::Options {
            font_resolver: resolver,
            fontdb: self.fontdb.clone(),
            resources_dir: None,
            // A fragment or malformed data URL must never become a cwd-relative
            // filename. usvg resolves supported scene fragments before this hook.
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_string: Box::new(|_, _| None),
                resolve_data: Box::new(|mime, data, options| {
                    // text/plain is also sniffed as SVG by usvg, except for these
                    // supported raster signatures. Other MIME types never parse XML.
                    let raster = data.starts_with(b"\x89PNG\r\n\x1a\n")
                        || data.starts_with(b"\xff\xd8")
                        || data.starts_with(b"GIF87a")
                        || data.starts_with(b"GIF89a")
                        || (data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP"));
                    if mime != "image/svg+xml" && (mime != "text/plain" || raster) {
                        return data_resolver(mime, data, options);
                    }
                    let (parent_depth, layers) = {
                        let state = resource_state.lock().unwrap();
                        (state.0, state.1)
                    };
                    let checked = if layers >= 4 {
                        Err(GuardError(
                            "SVG exceeds the embedded SVG nesting limit (4)".into(),
                        ))
                    } else {
                        xml::render_depth(&data, parent_depth)
                    };
                    let depth = match checked {
                        Ok(depth) => depth,
                        Err(error) => {
                            resource_state.lock().unwrap().2 = Some(error);
                            return None;
                        }
                    };
                    {
                        let mut state = resource_state.lock().unwrap();
                        state.0 = depth;
                        state.1 = layers + 1;
                    }
                    let image = data_resolver(mime, data, options);
                    let mut state = resource_state.lock().unwrap();
                    state.0 = parent_depth;
                    state.1 = layers;
                    image
                }),
            },
            ..usvg::Options::default()
        };
        let source = xml::renderer_source(source)?;
        let tree = usvg::Tree::from_data(&source, &options)
            .map_err(|error| GuardError(format!("cannot parse SVG for rendering: {error}")))?;
        if let Some(error) = resource_state.lock().unwrap().2.take() {
            return Err(error);
        }
        let issues = issues.lock().unwrap();
        if !issues.is_empty() {
            return Err(GuardError(format!(
                "isolated fonts: {}; supply matching fonts with --font-dir",
                issues.join("; ")
            )));
        }
        if self.isolated_fonts {
            validate_glyphs(tree.root())?;
        }
        Ok(tree)
    }

    fn render(&self, source: &[u8], canvas: Option<(u32, u32)>) -> Result<Pixmap, GuardError> {
        self.render_tree(&self.parse(source)?, canvas)
    }

    fn render_tree(
        &self,
        tree: &usvg::Tree,
        canvas: Option<(u32, u32)>,
    ) -> Result<Pixmap, GuardError> {
        // Keep the original raster extent without changing SVG viewport coordinates.
        let (width, height) = canvas.unwrap_or_else(|| {
            (
                (tree.size().width() * self.scale).ceil() as u32,
                (tree.size().height() * self.scale).ceil() as u32,
            )
        });
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(GuardError(
                "rendered SVG exceeds the pixel limit".to_string(),
            ));
        }
        let mut pixmap = Pixmap::new(width, height)
            .ok_or_else(|| GuardError("cannot allocate SVG render buffer".to_string()))?;
        resvg::render(
            tree,
            Transform::from_scale(self.scale, self.scale),
            &mut pixmap.as_mut(),
        );
        Ok(pixmap)
    }
}

// A fallback callback can succeed while usvg abandons reshaping. Check the
// actual positioned output, not the availability of a candidate font.
fn validate_glyphs(group: &usvg::Group) -> Result<(), GuardError> {
    for node in group.children() {
        match node {
            usvg::Node::Text(text) => {
                for span in text.layouted() {
                    for glyph in &span.positioned_glyphs {
                        if glyph.id.0 == 0 {
                            let codepoints = if glyph.text.is_empty() {
                                "cluster not attributed by renderer".to_string()
                            } else {
                                glyph
                                    .text
                                    .chars()
                                    .map(|ch| format!("U+{:04X}", u32::from(ch)))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            };
                            return Err(GuardError(format!(
                                "isolated fonts: unresolved shaped glyph ({codepoints}); supply a font covering the entire text run with --font-dir or simplify its shaping"
                            )));
                        }
                    }
                }
            }
            usvg::Node::Group(child) => validate_glyphs(child)?,
            _ => {}
        }
        let mut result = Ok(());
        node.subroots(|root| {
            if result.is_ok() {
                result = validate_glyphs(root);
            }
        });
        result?;
    }
    Ok(())
}

// Source variants cannot preserve effects whose coordinates depend on the
// original group bounds. usvg's writer serializes their resolved geometry.
fn has_scene_effects(group: &usvg::Group) -> bool {
    group.clip_path().is_some()
        || group.mask().is_some()
        || !group.filters().is_empty()
        || group.children().iter().any(|node| match node {
            usvg::Node::Group(child) => has_scene_effects(child),
            _ => false,
        })
}

/// Analyze SVG input, returning advisory warnings or an input, font, resource, or unsupported-analysis error.
pub fn analyze_file(path: &Path, config: &Config) -> Result<Report, GuardError> {
    analyze_with_mask_budget(path, config, MAX_TEXT_MASK_BYTES)
}

fn analyze_with_mask_budget(
    path: &Path,
    config: &Config,
    mask_budget: usize,
) -> Result<Report, GuardError> {
    validate_config(config)?;
    let file = fs::File::open(path)
        .map_err(|error| GuardError(format!("cannot read {}: {error}", path.display())))?;
    let source = read_svg(file, config.max_svg_bytes)
        .map_err(|error| GuardError(format!("cannot read {}: {error}", path.display())))?;
    let mut annotated = xml::annotate(&source)?;
    let renderer = Renderer::new(config);
    let tree = renderer.parse(&annotated.source)?;
    let final_image = renderer.render_tree(&tree, None)?;
    let canvas = Some((final_image.width(), final_image.height()));
    if has_scene_effects(tree.root()) || xml::has_local_references(&annotated.source)? {
        // Resolve instance inheritance, dependencies and original effect bounds
        // before any analysis rewrite, even when no href appears in the source.
        let resolved = tree.to_string(&usvg::WriteOptions {
            preserve_text: true,
            ..usvg::WriteOptions::default()
        });
        let resolved_image = renderer.render(resolved.as_bytes(), canvas)?;
        if resolved_image.data() != final_image.data() {
            return Err(GuardError("unsupported scene analysis: renderer serialization changes pixels; simplify the artwork or its text/effects".into()));
        }
        annotated = xml::annotate(resolved.as_bytes())?;
    }
    drop(tree);
    let variant_source = style::normalize(&annotated.source)?;
    let mut warnings = Vec::new();
    let mut regions = Vec::new();
    let mut retained_mask_bytes = 0;

    if annotated.texts.is_empty() {
        warnings.push(warning(
            "no-semantic-text",
            "No visible semantic <text> elements were found.",
            None,
            None,
            None,
        ));
    }

    for meta in &annotated.texts {
        // PIRA: bound full-canvas text masks, not total RSS. Cropped masks are
        // a possible upgrade if this conservative allocation ceiling is too low.
        let next_mask_bytes =
            checked_mask_bytes(retained_mask_bytes, final_image.data().len(), mask_budget)?;
        let isolated = xml::rewrite(&variant_source, &meta.key, Variant::IsolateText)?;
        // Sequential probes preserve the retained-plus-next-mask resource envelope.
        let unclipped = xml::rewrite(&isolated, "", Variant::Unclip)?;
        let unclipped_alpha = alpha_sum(&renderer.render(&unclipped, canvas)?);
        let mask = renderer.render(&isolated, canvas)?;
        if unclipped_alpha > alpha_sum(&mask) {
            warnings.push(warning(
                "text-clipped",
                "Clipping or masking removes rendered glyph pixels.",
                Some(meta.text.clone()),
                Some(meta.label()),
                None,
            ));
        }
        ensure_same_size(&final_image, &mask)?;
        let maximum_alpha = mask
            .data()
            .chunks_exact(4)
            .map(|pixel| pixel[3])
            .max()
            .unwrap_or(0);
        if maximum_alpha < 8 {
            warnings.push(warning(
                "text-not-rendered",
                "Text produced no measurable filled glyph pixels; it may be hidden, stroke-only, or use an unsupported style.",
                Some(meta.text.clone()),
                Some(meta.label()),
                None,
            ));
            continue;
        }

        // The real painted mask above measures loss/absence, but its element-wide
        // alpha cannot define glyph cores when spans have different opacities.
        // Drop it before the coverage render to preserve retained-plus-next bytes.
        drop(mask);
        let coverage = xml::rewrite(&isolated, "", Variant::TextCoverage)?;
        let mask = renderer.render(&coverage, canvas)?;
        let maximum_alpha = mask
            .data()
            .chunks_exact(4)
            .map(|pixel| pixel[3])
            .max()
            .unwrap_or(0);
        if maximum_alpha < 8 {
            return Err(GuardError(
                "unsupported scene analysis: glyph coverage probe lost painted text".into(),
            ));
        }
        let core_threshold = 24_u8
            .max((f32::from(maximum_alpha) * 0.7).round() as u8)
            .min(maximum_alpha);
        let Some(glyph_bounds) = alpha_bounds(&mask, core_threshold) else {
            continue;
        };
        let padding = ((f64::from(glyph_bounds.height()) * config.block_padding_fraction).round()
            as u32)
            .max(1);
        let protected_bounds = glyph_bounds.expanded(padding, mask.width(), mask.height());

        if touches_canvas_edge(glyph_bounds, mask.width(), mask.height()) {
            warnings.push(warning(
                "text-at-viewport-edge",
                "Text touches the SVG viewport and may be cropped.",
                Some(meta.text.clone()),
                Some(meta.label()),
                None,
            ));
        }

        let removed_source = xml::rewrite(&variant_source, &meta.key, Variant::Remove)?;
        let without_text = renderer.render(
            &removed_source,
            Some((final_image.width(), final_image.height())),
        )?;
        ensure_same_size(&final_image, &without_text)?;
        let (low_fraction, minimum) = contrast_summary(
            &final_image,
            &without_text,
            &mask,
            core_threshold,
            config.min_contrast,
        );
        if low_fraction > config.max_low_contrast_fraction {
            warnings.push(warning(
                "low-contrast",
                &format!(
                    "{:.1}% of glyph-core pixels are below {:.1}:1 contrast (minimum {minimum:.2}:1, assuming a white canvas).",
                    low_fraction * 100.0,
                    config.min_contrast,
                ),
                Some(meta.text.clone()),
                Some(meta.label()),
                None,
            ));
        }

        retained_mask_bytes = next_mask_bytes;
        regions.push(TextRegion {
            meta: meta.clone(),
            mask,
            core_threshold,
            glyph_bounds,
            protected_bounds,
        });
    }

    warnings.extend(text_overlap_warnings(&regions));
    warnings.extend(stroke_intrusion_warnings(
        &annotated,
        &variant_source,
        &regions,
        &final_image,
        &renderer,
        config,
    )?);
    deduplicate(&mut warnings);
    let status = if warnings.is_empty() {
        "clear"
    } else {
        "warnings"
    };
    Ok(Report {
        source: path.display().to_string(),
        renderer: "resvg 0.48.1".to_string(),
        status,
        warnings,
    })
}

fn alpha_sum(image: &Pixmap) -> u64 {
    image
        .data()
        .chunks_exact(4)
        .map(|pixel| u64::from(pixel[3]))
        .sum()
}

fn checked_mask_bytes(retained: usize, next: usize, budget: usize) -> Result<usize, GuardError> {
    retained
        .checked_add(next)
        .filter(|total| *total <= budget)
        .ok_or_else(|| GuardError(format!("SVG exceeds the text-mask budget ({budget} bytes)")))
}

fn read_svg(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, GuardError> {
    let mut source = Vec::new();
    reader
        .by_ref()
        .take(limit as u64)
        .read_to_end(&mut source)
        .map_err(|error| GuardError(error.to_string()))?;
    // Probe separately so even usize::MAX does not overflow the limit.
    let mut extra = [0_u8];
    match reader.read_exact(&mut extra) {
        Ok(()) => return Err(GuardError("SVG exceeds the input-size limit".to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
        Err(error) => return Err(GuardError(error.to_string())),
    }
    Ok(source)
}

fn validate_config(config: &Config) -> Result<(), GuardError> {
    if config.isolated_fonts {
        if config.font_dirs.is_empty() {
            return Err(GuardError(
                "--isolated-fonts requires at least one --font-dir".into(),
            ));
        }
        for directory in &config.font_dirs {
            fs::read_dir(directory).map_err(|error| {
                GuardError(format!(
                    "cannot read isolated font directory {}: {error}",
                    directory.display()
                ))
            })?;
        }
    }
    if !config.block_padding_fraction.is_finite() || config.block_padding_fraction < 0.0 {
        return Err(GuardError(
            "block padding fraction must be finite and nonnegative".to_string(),
        ));
    }
    if !config.scale.is_finite() || config.scale <= 0.0 {
        return Err(GuardError("scale must be positive".to_string()));
    }
    if !(1.0..=21.0).contains(&config.min_contrast) {
        return Err(GuardError(
            "minimum contrast must be between 1 and 21".to_string(),
        ));
    }
    if !(0.0..=1.0).contains(&config.max_low_contrast_fraction)
        || !(0.0..=1.0).contains(&config.crossing_ratio)
    {
        return Err(GuardError(
            "fraction options must be between 0 and 1".to_string(),
        ));
    }
    Ok(())
}

fn stroke_intrusion_warnings(
    annotated: &AnnotatedSvg,
    variant_source: &[u8],
    regions: &[TextRegion],
    final_image: &Pixmap,
    renderer: &Renderer,
    config: &Config,
) -> Result<Vec<GuardWarning>, GuardError> {
    let mut warnings = Vec::new();
    if regions.is_empty() {
        return Ok(warnings);
    }
    for candidate in &annotated.candidates {
        let isolated_source = xml::rewrite(variant_source, &candidate.key, Variant::IsolateStroke)?;
        let stroke = renderer.render(
            &isolated_source,
            Some((final_image.width(), final_image.height())),
        )?;
        let Some(stroke_bounds) = alpha_bounds(&stroke, 24) else {
            continue;
        };
        let nearby: Vec<_> = regions
            .iter()
            .filter(|region| stroke_bounds.intersects(region.protected_bounds))
            .collect();
        if nearby.is_empty() {
            continue;
        }
        let removed_source = xml::rewrite(variant_source, &candidate.key, Variant::Remove)?;
        let without_candidate = renderer.render(
            &removed_source,
            Some((final_image.width(), final_image.height())),
        )?;
        ensure_same_size(final_image, &without_candidate)?;
        for region in nearby {
            let visible = visible_stroke_points(
                &stroke,
                final_image,
                &without_candidate,
                region.protected_bounds,
            );
            if visible.is_empty() {
                continue;
            }
            let direct = visible
                .iter()
                .any(|&(x, y)| alpha_at(&region.mask, x, y) >= region.core_threshold);
            let crossing = crosses_text_block(&visible, region.glyph_bounds, config.crossing_ratio);
            if direct || crossing {
                let reason = if direct {
                    "intersects glyph-core pixels"
                } else {
                    "traverses the protected text block"
                };
                warnings.push(warning(
                    "stroke-intrusion",
                    &format!("A visible stroked element {reason}."),
                    Some(region.meta.text.clone()),
                    Some(region.meta.label()),
                    Some(candidate.label()),
                ));
            }
        }
    }
    Ok(warnings)
}

fn visible_stroke_points(
    stroke: &Pixmap,
    final_image: &Pixmap,
    without_candidate: &Pixmap,
    bounds: Bounds,
) -> Vec<(u32, u32)> {
    let mut points = Vec::new();
    for y in bounds.y0..bounds.y1 {
        for x in bounds.x0..bounds.x1 {
            if alpha_at(stroke, x, y) < 24 {
                continue;
            }
            let final_rgb = rgb_on_white(final_image, x, y);
            let removed_rgb = rgb_on_white(without_candidate, x, y);
            let delta = final_rgb
                .into_iter()
                .zip(removed_rgb)
                .map(|(left, right)| left.abs_diff(right))
                .max()
                .unwrap_or(0);
            if delta >= 8 {
                points.push((x, y));
            }
        }
    }
    points
}

fn crosses_text_block(points: &[(u32, u32)], glyphs: Bounds, ratio: f64) -> bool {
    let min_x = points.iter().map(|point| point.0).min().unwrap_or(0);
    let max_x = points.iter().map(|point| point.0).max().unwrap_or(0);
    let min_y = points.iter().map(|point| point.1).min().unwrap_or(0);
    let max_y = points.iter().map(|point| point.1).max().unwrap_or(0);
    let span_x = max_x.saturating_sub(min_x) + 1;
    let span_y = max_y.saturating_sub(min_y) + 1;
    f64::from(span_x) >= f64::from(glyphs.width()) * ratio
        || f64::from(span_y) >= f64::from(glyphs.height()) * ratio
}

fn text_overlap_warnings(regions: &[TextRegion]) -> Vec<GuardWarning> {
    let mut warnings = Vec::new();
    for (index, first) in regions.iter().enumerate() {
        for second in &regions[index + 1..] {
            if !first.protected_bounds.intersects(second.protected_bounds) {
                continue;
            }
            let Some(overlap) = intersection(first.glyph_bounds, second.glyph_bounds) else {
                continue;
            };
            let mut direct = false;
            'rows: for y in overlap.y0..overlap.y1 {
                for x in overlap.x0..overlap.x1 {
                    if alpha_at(&first.mask, x, y) >= first.core_threshold
                        && alpha_at(&second.mask, x, y) >= second.core_threshold
                    {
                        direct = true;
                        break 'rows;
                    }
                }
            }
            if direct {
                warnings.push(warning(
                    "text-overlap",
                    &format!(
                        "Glyph-core pixels overlap text element {}.",
                        second.meta.label()
                    ),
                    Some(first.meta.text.clone()),
                    Some(first.meta.label()),
                    Some(second.meta.label()),
                ));
            }
        }
    }
    warnings
}

fn contrast_summary(
    final_image: &Pixmap,
    background: &Pixmap,
    text_mask: &Pixmap,
    threshold: u8,
    minimum_required: f64,
) -> (f64, f64) {
    let mut low = 0_u64;
    let mut total = 0_u64;
    let mut minimum = 21.0_f64;
    for y in 0..final_image.height() {
        for x in 0..final_image.width() {
            if alpha_at(text_mask, x, y) < threshold {
                continue;
            }
            let ratio = contrast_ratio(
                rgb_on_white(final_image, x, y),
                rgb_on_white(background, x, y),
            );
            total += 1;
            minimum = minimum.min(ratio);
            if ratio < minimum_required {
                low += 1;
            }
        }
    }
    (
        if total == 0 {
            1.0
        } else {
            low as f64 / total as f64
        },
        minimum,
    )
}

fn contrast_ratio(first: [u8; 3], second: [u8; 3]) -> f64 {
    let first = relative_luminance(first);
    let second = relative_luminance(second);
    let lighter = first.max(second);
    let darker = first.min(second);
    (lighter + 0.05) / (darker + 0.05)
}

fn relative_luminance(color: [u8; 3]) -> f64 {
    let channel = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color[0]) + 0.7152 * channel(color[1]) + 0.0722 * channel(color[2])
}

fn rgb_on_white(image: &Pixmap, x: u32, y: u32) -> [u8; 3] {
    let offset = ((y * image.width() + x) * 4) as usize;
    let pixel = &image.data()[offset..offset + 4];
    let inverse_alpha = 255_u8.saturating_sub(pixel[3]);
    [
        pixel[0].saturating_add(inverse_alpha),
        pixel[1].saturating_add(inverse_alpha),
        pixel[2].saturating_add(inverse_alpha),
    ]
}

fn alpha_at(image: &Pixmap, x: u32, y: u32) -> u8 {
    image.data()[((y * image.width() + x) * 4 + 3) as usize]
}

fn alpha_bounds(image: &Pixmap, threshold: u8) -> Option<Bounds> {
    let mut x0 = image.width();
    let mut y0 = image.height();
    let mut x1 = 0;
    let mut y1 = 0;
    let mut found = false;
    for y in 0..image.height() {
        for x in 0..image.width() {
            if alpha_at(image, x, y) >= threshold {
                found = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    found.then_some(Bounds { x0, y0, x1, y1 })
}

fn intersection(first: Bounds, second: Bounds) -> Option<Bounds> {
    let bounds = Bounds {
        x0: first.x0.max(second.x0),
        y0: first.y0.max(second.y0),
        x1: first.x1.min(second.x1),
        y1: first.y1.min(second.y1),
    };
    (bounds.x0 < bounds.x1 && bounds.y0 < bounds.y1).then_some(bounds)
}

fn touches_canvas_edge(bounds: Bounds, width: u32, height: u32) -> bool {
    bounds.x0 == 0 || bounds.y0 == 0 || bounds.x1 == width || bounds.y1 == height
}

fn ensure_same_size(first: &Pixmap, second: &Pixmap) -> Result<(), GuardError> {
    if first.width() != second.width() || first.height() != second.height() {
        return Err(GuardError(
            "renderer returned inconsistent canvas dimensions".to_string(),
        ));
    }
    Ok(())
}

fn warning(
    code: &str,
    message: &str,
    text: Option<String>,
    text_element: Option<String>,
    interfering_element: Option<String>,
) -> GuardWarning {
    GuardWarning {
        code: code.to_string(),
        message: message.to_string(),
        text,
        text_element,
        interfering_element,
    }
}

fn deduplicate(warnings: &mut Vec<GuardWarning>) {
    let mut seen = HashSet::new();
    warnings.retain(|item| seen.insert(item.clone()));
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    match real_run(args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("pira_svg_check: error: {error}");
            2
        }
    }
}

fn real_run(args: impl IntoIterator<Item = OsString>) -> Result<i32, GuardError> {
    let mut args = args.into_iter();
    let _program = args.next();
    let mut config = Config::default();
    let mut json = false;
    let mut source = None;
    let mut pending: Option<String> = None;

    for argument in args {
        if let Some(option) = pending.take() {
            if option == "--font-dir" {
                config.font_dirs.push(PathBuf::from(argument));
                continue;
            }
            let value = argument
                .to_str()
                .ok_or_else(|| GuardError(format!("{option} value is not valid UTF-8")))?;
            match option.as_str() {
                "--scale" => config.scale = parse_number(value, &option)?,
                "--min-contrast" => config.min_contrast = parse_number(value, &option)?,
                "--crossing-ratio" => config.crossing_ratio = parse_number(value, &option)?,
                _ => unreachable!(),
            }
            continue;
        }
        let Some(value) = argument.to_str() else {
            if source.is_none() {
                source = Some(PathBuf::from(&argument));
                continue;
            }
            return Err(GuardError("unexpected non-UTF-8 argument".to_string()));
        };
        match value {
            "--help" | "-h" => {
                print_help();
                return Ok(0);
            }
            "--version" | "-V" => {
                println!("pira_svg_check {VERSION}");
                return Ok(0);
            }
            "--json" => json = true,
            "--isolated-fonts" => config.isolated_fonts = true,
            "--scale" | "--min-contrast" | "--crossing-ratio" | "--font-dir" => {
                pending = Some(value.to_string())
            }
            _ if value.starts_with('-') => {
                return Err(GuardError(format!("unknown option: {value}")));
            }
            _ if source.is_none() => source = Some(PathBuf::from(&argument)),
            _ => return Err(GuardError("only one SVG input may be provided".to_string())),
        }
    }
    if let Some(option) = pending {
        return Err(GuardError(format!("missing value for {option}")));
    }
    let source =
        source.ok_or_else(|| GuardError("missing SVG input; use --help for usage".to_string()))?;
    let report = analyze_file(&source, &config)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| GuardError(format!("cannot serialize report: {error}")))?
        );
    } else if report.warnings.is_empty() {
        println!("pira_svg_check: clear (no warnings)");
    } else {
        println!("pira_svg_check: {} warning(s)", report.warnings.len());
        for item in report.warnings {
            let subject = item
                .text_element
                .map(|value| format!(" {value}"))
                .unwrap_or_default();
            let interference = item
                .interfering_element
                .map(|value| format!("; interfering element {value}"))
                .unwrap_or_default();
            println!(
                "- [{}]{}: {}{}",
                item.code, subject, item.message, interference
            );
        }
    }
    Ok(0)
}

fn parse_number<T>(value: &str, option: &str) -> Result<T, GuardError>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| GuardError(format!("invalid numeric value for {option}: {value}")))
}

fn print_help() {
    println!(
        "pira_svg_check {VERSION} — conservative warning-only PIRA SVG check\n\n\
USAGE\n  pira_svg_check [OPTIONS] SVG\n\n\
OPTIONS\n  --json                 Emit a machine-readable report\n  --scale NUMBER         Fixed rasterization scale [default: 2]\n  --min-contrast NUMBER  Minimum text/background contrast [default: 4.5]\n  --crossing-ratio N     Fraction of a text block a stroke must traverse [default: 0.45]\n  --font-dir DIR         Add a font directory; repeatable\n  --isolated-fonts       Use only supplied font directories; missing fonts fail\n  -h, --help             Show this help\n  -V, --version          Show the version\n\n\
Warnings are advisory and return exit code 0. Analysis errors return exit code 2.\n\
System fonts are loaded by default. Unsupported scene analysis and missing isolated fonts are errors."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn accepted_render_depth_keeps_pixels_and_unsupported_encodings_fail() {
        let renderer = Renderer::new(&Config {
            scale: 1.0,
            ..Config::default()
        });
        let source = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>{}<rect width='10' height='10'/>{}</svg>",
            "<g>".repeat(62),
            "</g>".repeat(62)
        );
        let image = renderer.render(source.as_bytes(), None).unwrap();
        assert_eq!(image.pixel(5, 5).unwrap().alpha(), 255);
        assert_eq!(rgb_on_white(&image, 5, 5), [0, 0, 0]);
        assert!(
            xml::render_depth(b"\x1f\x8b", 0)
                .unwrap_err()
                .to_string()
                .contains("compressed SVG")
        );
        assert!(
            xml::render_depth(b"<!DOCTYPE svg><svg/>", 0)
                .unwrap_err()
                .to_string()
                .contains("document types")
        );
    }

    #[test]
    fn coverage_pixels_are_geometric_but_contrast_colors_remain_faint() {
        let renderer = Renderer::new(&Config::default());
        for family in ["sans-serif", "monospace"] {
            let svg = |paint: &str| {
                format!(
                    "<svg xmlns='http://www.w3.org/2000/svg' width='500' height='100'><text x='20' y='50' font-family='{family}' font-size='28'>Solid<tspan x='180' {paint}>Faint label</tspan></text></svg>"
                )
            };
            let opaque = renderer.render(svg("").as_bytes(), None).unwrap();
            // usvg ignores opacity on tspan; do not impose browser semantics.
            assert_eq!(
                opaque.data(),
                renderer
                    .render(svg("opacity='.5'").as_bytes(), None)
                    .unwrap()
                    .data()
            );
            for paint in [
                "fill-opacity='.1'",
                "fill-opacity='.5'",
                "style='fill-opacity:.1!important'",
            ] {
                let source = xml::annotate(svg(paint).as_bytes()).unwrap();
                let normalized = style::normalize(&source.source).unwrap();
                let isolated =
                    xml::rewrite(&normalized, &source.texts[0].key, Variant::IsolateText).unwrap();
                let actual = renderer.render(&isolated, None).unwrap();
                let geometric = renderer
                    .render(
                        &xml::rewrite(&isolated, "", Variant::TextCoverage).unwrap(),
                        None,
                    )
                    .unwrap();
                assert_eq!(opaque.data(), geometric.data(), "{family} {paint}");
                let mut cores = 0;
                for y in 0..opaque.height() {
                    for x in 360..opaque.width() {
                        if alpha_at(&opaque, x, y) >= 179 {
                            cores += 1;
                            assert!(contrast_ratio(rgb_on_white(&actual, x, y), [255; 3]) < 4.5);
                        }
                    }
                }
                assert!(
                    cores > 100,
                    "font must produce substantial measurable faint glyphs"
                );
            }
        }
    }

    #[test]
    fn image_data_and_scene_fragments_keep_real_pixels() {
        let renderer = Renderer::new(&Config {
            scale: 1.0,
            ..Config::default()
        });
        for href in [
            "data:image/svg+xml;base64,PHN2ZyB4bWxucz0naHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmcnIHdpZHRoPScxMCcgaGVpZ2h0PScxMCc+PHJlY3Qgd2lkdGg9JzEwJyBoZWlnaHQ9JzEwJy8+PC9zdmc+",
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAX+XDSwAAAABJRU5ErkJggg==",
        ] {
            for href in [
                href.to_string(),
                format!("data:text/plain;base64,{}", href.split_once(',').unwrap().1),
            ] {
                for body in [
                    format!("<image href='{href}' width='10' height='10'/>"),
                    format!(
                        "<defs><filter id='f' x='0' y='0' width='10' height='10' filterUnits='userSpaceOnUse'><feImage href='{href}'/></filter></defs><rect width='10' height='10' fill='red' filter='url(#f)'/>"
                    ),
                ] {
                    let source = format!(
                        "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>{body}</svg>"
                    );
                    let image = renderer.render(source.as_bytes(), None).unwrap();
                    assert_eq!(image.pixel(5, 5).unwrap().alpha(), 255, "{body}");
                    assert_eq!(rgb_on_white(&image, 5, 5), [0, 0, 0], "{body}");
                }
            }
        }
        // feImage supports scene fragments independently of the image resolver.
        let source = b"<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><defs><rect id='paint' width='10' height='10'/><filter id='f' x='0' y='0' width='10' height='10' filterUnits='userSpaceOnUse'><feImage href='#paint'/></filter></defs><rect width='10' height='10' fill='red' filter='url(#f)'/></svg>";
        let image = renderer.render(source, None).unwrap();
        assert_eq!(image.pixel(5, 5).unwrap().alpha(), 255);
        assert_eq!(rgb_on_white(&image, 5, 5), [0, 0, 0]);
    }

    #[test]
    fn renderer_pixels_match_inline_and_nested_transformed_instances() {
        let renderer = Renderer::new(&Config::default());
        for paint in [
            "fill='#ddd'",
            "fill='black' stroke='black' stroke-width='1'",
        ] {
            let content = format!(
                "<text x='20' y='45' font-size='24' {paint}>Visible label</text><path d='M10 55H200' stroke='black'/>"
            );
            let wrap = |body: &str| {
                format!(
                    "<svg xmlns='http://www.w3.org/2000/svg' width='400' height='220'>{body}</svg>"
                )
            };
            let inline = wrap(&format!(
                "<g transform='translate(4 3) rotate(5)'>{content}</g><g transform='translate(5 90) scale(1.2)'>{content}</g>"
            ));
            let references = wrap(&format!(
                "<defs><g id='label'>{content}</g><g id='nested'><use href='#label'/></g></defs><use href='#nested' transform='translate(4 3) rotate(5)'/><use href='#nested' transform='translate(5 90) scale(1.2)'/>"
            ));
            let original = renderer.render(references.as_bytes(), None).unwrap();
            assert_eq!(
                original.data(),
                renderer.render(inline.as_bytes(), None).unwrap().data()
            );
            let resolved =
                renderer
                    .parse(references.as_bytes())
                    .unwrap()
                    .to_string(&usvg::WriteOptions {
                        preserve_text: true,
                        ..Default::default()
                    });
            assert_eq!(
                original.data(),
                renderer.render(resolved.as_bytes(), None).unwrap().data()
            );
        }
    }

    #[test]
    fn normalized_author_styles_preserve_renderer_pixels() {
        let renderer = Renderer::new(&Config {
            scale: 1.0,
            ..Config::default()
        });
        for rules in [
            "* {fill:red!important} #first {fill:blue!important} rect {fill:green}",
            "rect {fill:red} .paint {fill:blue} #first {fill:green} rect {fill:black}",
            "g {fill:blue} rect {fill:inherit} g > rect:first-child {fill:red} rect + rect {fill:green}",
            "[data-note='A B'] {fill:red} [data-note='A&#9;B'] {fill:green}",
            "text {font: italic 18px serif; font-size:22px; fill:blue} text {font-size:14px!important; font:24px serif}",
        ] {
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='160' height='80'><style>{rules}</style><g><rect id='first' class='paint' data-note='A&#9;B' x='5' y='5' width='30' height='30'/><rect data-note='A\tB' x='45' y='5' width='30' height='30' style='fill:purple!important;fill:orange!important'/></g><text x='10' y='65'>Words</text></svg>"
            );
            let original = renderer.render(source.as_bytes(), None).unwrap();
            let normalized = style::normalize(source.as_bytes()).unwrap();
            let rendered = renderer.render(&normalized, None).unwrap();
            assert_eq!(original.data(), rendered.data(), "{rules}");
        }
    }

    #[test]
    fn analysis_overrides_are_node_and_property_scoped() {
        let renderer = Renderer::new(&Config {
            scale: 1.0,
            ..Config::default()
        });
        for rules in [
            "* {display:inline!important;fill:red!important} #target {filter:url(#gone)!important}",
            "rect {display:inline!important;fill:red!important} #target {filter:url(#gone)!important}",
            "#target {display:inline!important;fill:red!important;filter:url(#gone)!important}",
        ] {
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='60'><defs><filter id='gone'><feComponentTransfer><feFuncA type='linear' slope='0'/></feComponentTransfer></filter></defs><style>{rules}</style><rect id='target' x='10' y='10' width='20' height='20' stroke='black' stroke-width='2' style='display:inline!important;fill:blue!important'/><rect id='other' x='60' y='10' width='20' height='20' fill='green'/></svg>"
            );
            let annotated = xml::annotate(source.as_bytes()).unwrap();
            let normalized = style::normalize(&annotated.source).unwrap();
            let key = &annotated
                .candidates
                .iter()
                .find(|meta| meta.id.as_deref() == Some("target"))
                .unwrap()
                .key;
            let isolated = xml::rewrite(&normalized, key, Variant::IsolateStroke).unwrap();
            let isolated = renderer.render(&isolated, None).unwrap();
            assert!(
                alpha_at(&isolated, 10, 20) > 0,
                "filter was not overridden: {rules}"
            );
            assert_eq!(
                alpha_at(&isolated, 20, 20),
                0,
                "fill was not overridden: {rules}"
            );
            assert_eq!(
                alpha_at(&isolated, 70, 20),
                0,
                "other node was not isolated: {rules}"
            );
            let original = renderer.render(&annotated.source, None).unwrap();
            let removed = xml::rewrite(&normalized, key, Variant::Remove).unwrap();
            let removed = renderer.render(&removed, None).unwrap();
            assert_eq!(alpha_at(&removed, 10, 20), 0);
            assert_eq!(
                removed.pixel(70, 20),
                original.pixel(70, 20),
                "unrelated node changed: {rules}"
            );
        }
    }

    #[test]
    fn fixed_canvas_preserves_coordinates_and_pixel_limit() {
        let renderer = super::Renderer::new(&Config {
            scale: 1.0,
            ..Config::default()
        });
        let source = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect x="20" y="10" width="10" height="10"/></svg>"#;
        let original = renderer.render(source, None).unwrap();
        let padded = renderer.render(source, Some((100, 100))).unwrap();
        for y in 0..original.height() {
            for x in 0..original.width() {
                assert_eq!(
                    super::alpha_at(&original, x, y),
                    super::alpha_at(&padded, x, y)
                );
            }
        }
        assert_eq!(super::alpha_at(&padded, 25, 15), 255);
        assert_eq!(super::alpha_at(&padded, 5, 5), 0);
        assert!(
            renderer
                .render(source, Some((super::MAX_PIXELS as u32 + 1, 1)))
                .is_err()
        );
    }

    #[test]
    fn input_read_stops_after_limit_plus_one_byte() {
        let mut reader = Cursor::new([b'x'; 100]);
        assert!(
            read_svg(&mut reader, 4)
                .unwrap_err()
                .to_string()
                .contains("input-size")
        );
        assert_eq!(reader.position(), 5);
    }

    #[test]
    fn input_read_accepts_exact_empty_and_maximum_limits() {
        assert_eq!(read_svg(&b"abc"[..], 3).unwrap(), b"abc");
        assert_eq!(read_svg(&b"abc"[..], usize::MAX).unwrap(), b"abc");
        assert!(read_svg(&b""[..], 0).unwrap().is_empty());
        assert!(read_svg(&b"x"[..], 0).is_err());
    }

    #[test]
    fn input_read_propagates_failure() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("probe read failure"))
            }
        }
        assert!(
            read_svg(Broken, 4)
                .unwrap_err()
                .to_string()
                .contains("probe read failure")
        );
        assert!(
            read_svg(Broken, 0)
                .unwrap_err()
                .to_string()
                .contains("probe read failure")
        );
    }

    #[test]
    fn mask_accounting_checks_boundary_and_overflow() {
        assert_eq!(checked_mask_bytes(4, 4, 8).unwrap(), 8);
        assert!(checked_mask_bytes(4, 4, 7).is_err());
        assert!(checked_mask_bytes(usize::MAX, 1, usize::MAX).is_err());
    }

    // Mask accounting depends on visible glyphs, not the host's default family
    // alias. This selects actual installed faces; it never skips the test.
    fn with_available_font(source: &str, renderer: &Renderer) -> Result<String, String> {
        for face in renderer.fontdb.faces() {
            for (family, _) in &face.families {
                let family = quick_xml::escape::escape(family);
                let source = source.replace("<text ", &format!("<text font-family=\"{family}\" "));
                if let Ok(image) = renderer.render(source.as_bytes(), None)
                    && alpha_sum(&image) > 0
                {
                    return Ok(source);
                }
            }
        }
        Err(format!(
            "SVG render tests require a usable installed font: loaded faces={}, default family={}, serif mapping={}. On Debian/Ubuntu provision fontconfig and fonts-dejavu-core before cargo test; do not skip mask-budget tests.",
            renderer.fontdb.len(),
            usvg::Options::default().font_family,
            renderer.fontdb.family_name(&usvg::fontdb::Family::Serif)
        ))
    }

    fn font_backed_budget_fixture(name: &str) -> PathBuf {
        let source = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap();
        let mut renderer = Renderer::new(&Config::default());
        // Validate the named family and final glyph coverage, not fallback to an
        // unrelated host default. The test database was intentionally discovered.
        renderer.isolated_fonts = true;
        let source =
            with_available_font(&source, &renderer).unwrap_or_else(|error| panic!("{error}"));
        let path =
            std::env::temp_dir().join(format!("pira-svg-budget-{}-{name}", std::process::id()));
        fs::write(&path, source).unwrap();
        path
    }

    #[test]
    fn font_prerequisite_distinguishes_empty_inventory_from_unmapped_defaults() {
        let source = include_str!("../../crates/pira_svg_check/tests/fixtures/two_labels.svg");
        let empty = Renderer {
            fontdb: Arc::new(usvg::fontdb::Database::new()),
            scale: 1.0,
            isolated_fonts: false,
        };
        assert_eq!(
            alpha_sum(&empty.render(source.as_bytes(), None).unwrap()),
            0
        );
        assert!(
            with_available_font(source, &empty)
                .unwrap_err()
                .contains("loaded faces=0")
        );

        let mut renderer = Renderer::new(&Config::default());
        let database = Arc::make_mut(&mut renderer.fontdb);
        let defaults: Vec<_> = database
            .faces()
            .filter(|face| {
                face.families
                    .iter()
                    .any(|(name, _)| name == "Times New Roman")
            })
            .map(|face| face.id)
            .collect();
        for id in defaults {
            database.remove_face(id);
        }
        database.set_serif_family("PiraUnavailableDefault");
        // Reproduce the same transparent masks with a NONEMPTY font database.
        // A font-count check alone would not establish the CI prerequisite.
        assert_eq!(
            alpha_sum(&renderer.render(source.as_bytes(), None).unwrap()),
            0
        );
        renderer.isolated_fonts = true;
        let explicit =
            with_available_font(source, &renderer).unwrap_or_else(|error| panic!("{error}"));
        assert!(!renderer.fontdb.is_empty());
        assert!(alpha_sum(&renderer.render(explicit.as_bytes(), None).unwrap()) > 0);
    }

    #[test]
    fn aggregate_mask_budget_is_enforced_in_analysis() {
        let path = font_backed_budget_fixture("two_labels.svg");
        let config = Config {
            scale: 1.0,
            ..Config::default()
        };
        let bytes = 120 * 60 * 4;
        let report = analyze_with_mask_budget(&path, &config, bytes * 2).unwrap();
        assert!(
            !report
                .warnings
                .iter()
                .any(|warning| warning.code == "text-not-rendered"),
            "font-backed labels must render before checking retained-mask accounting: {report:?}"
        );
        let error = analyze_with_mask_budget(&path, &config, bytes * 2 - 1).unwrap_err();
        assert!(error.to_string().contains("text-mask budget"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn discarded_masks_are_not_retained_but_still_require_allocation_room() {
        let path = font_backed_budget_fixture("hidden_labels.svg");
        let config = Config {
            scale: 1.0,
            ..Config::default()
        };
        let bytes = 120 * 60 * 4;
        let report = analyze_with_mask_budget(&path, &config, bytes * 2).unwrap();
        assert_eq!(
            report
                .warnings
                .iter()
                .filter(|w| w.code == "text-not-rendered")
                .count(),
            2
        );
        // The final hidden label cannot be known to be discardable until rendered.
        assert!(
            analyze_with_mask_budget(&path, &config, bytes)
                .unwrap_err()
                .to_string()
                .contains("text-mask budget")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn padding_requires_finite_nonnegative_values_without_upper_bound() {
        for padding in [-1.0, f64::NAN, f64::NEG_INFINITY, f64::INFINITY] {
            let config = Config {
                block_padding_fraction: padding,
                ..Config::default()
            };
            assert!(
                validate_config(&config)
                    .unwrap_err()
                    .to_string()
                    .contains("padding")
            );
        }
        for padding in [0.0, 0.12, 2.0, f64::MAX] {
            assert!(
                validate_config(&Config {
                    block_padding_fraction: padding,
                    ..Config::default()
                })
                .is_ok()
            );
        }
    }
}
