mod xml;

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;
use serde::Serialize;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::xml::{AnnotatedSvg, ElementMeta, Variant};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_PIXELS: u64 = 16_000_000;

#[derive(Clone, Debug)]
pub struct Config {
    pub scale: f32,
    pub min_contrast: f64,
    pub max_low_contrast_fraction: f64,
    pub crossing_ratio: f64,
    pub block_padding_fraction: f64,
    pub max_svg_bytes: usize,
    pub font_dirs: Vec<PathBuf>,
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
}

impl Renderer {
    fn new(config: &Config) -> Self {
        let mut database = usvg::fontdb::Database::new();
        database.load_system_fonts();
        for directory in &config.font_dirs {
            database.load_fonts_dir(directory);
        }
        Self {
            fontdb: Arc::new(database),
            scale: config.scale,
        }
    }

    fn render(&self, source: &[u8]) -> Result<Pixmap, GuardError> {
        let options = usvg::Options {
            fontdb: self.fontdb.clone(),
            resources_dir: None,
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_data(source, &options)
            .map_err(|error| GuardError(format!("cannot parse SVG for rendering: {error}")))?;
        let width = (tree.size().width() * self.scale).ceil() as u32;
        let height = (tree.size().height() * self.scale).ceil() as u32;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(GuardError(
                "rendered SVG exceeds the pixel limit".to_string(),
            ));
        }
        let mut pixmap = Pixmap::new(width, height)
            .ok_or_else(|| GuardError("cannot allocate SVG render buffer".to_string()))?;
        resvg::render(
            &tree,
            Transform::from_scale(self.scale, self.scale),
            &mut pixmap.as_mut(),
        );
        Ok(pixmap)
    }
}

pub fn analyze_file(path: &Path, config: &Config) -> Result<Report, GuardError> {
    validate_config(config)?;
    let source = fs::read(path)
        .map_err(|error| GuardError(format!("cannot read {}: {error}", path.display())))?;
    if source.len() > config.max_svg_bytes {
        return Err(GuardError("SVG exceeds the input-size limit".to_string()));
    }
    let annotated = xml::annotate(&source)?;
    let renderer = Renderer::new(config);
    let final_image = renderer.render(&annotated.source)?;
    let mut warnings = Vec::new();
    let mut regions = Vec::new();

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
        let isolated = xml::rewrite(&annotated.source, &meta.key, Variant::IsolateText)?;
        let mask = renderer.render(&isolated)?;
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

        let core_threshold = 24_u8.max((f32::from(maximum_alpha) * 0.7).round() as u8);
        let Some(glyph_bounds) = alpha_bounds(&mask, core_threshold) else {
            continue;
        };
        let padding = ((f64::from(glyph_bounds.height()) * config.block_padding_fraction).round()
            as u32)
            .max(1);
        let protected_bounds = glyph_bounds.expanded(padding, mask.width(), mask.height());

        if meta.clipped_or_masked {
            warnings.push(warning(
                "text-clipped",
                "Text or one of its ancestors uses clipping or masking and may lose glyph pixels.",
                Some(meta.text.clone()),
                Some(meta.label()),
                None,
            ));
        }
        if touches_canvas_edge(glyph_bounds, mask.width(), mask.height()) {
            warnings.push(warning(
                "text-at-viewport-edge",
                "Text touches the SVG viewport and may be cropped.",
                Some(meta.text.clone()),
                Some(meta.label()),
                None,
            ));
        }

        let removed_source = xml::rewrite(&annotated.source, &meta.key, Variant::Remove)?;
        let without_text = renderer.render(&removed_source)?;
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

fn validate_config(config: &Config) -> Result<(), GuardError> {
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
    regions: &[TextRegion],
    final_image: &Pixmap,
    renderer: &Renderer,
    config: &Config,
) -> Result<Vec<GuardWarning>, GuardError> {
    let mut warnings = Vec::new();
    for candidate in &annotated.candidates {
        let isolated_source =
            xml::rewrite(&annotated.source, &candidate.key, Variant::IsolateStroke)?;
        let stroke = renderer.render(&isolated_source)?;
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
        let removed_source = xml::rewrite(&annotated.source, &candidate.key, Variant::Remove)?;
        let without_candidate = renderer.render(&removed_source)?;
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
    let central_y0 = f64::from(glyphs.y0) + f64::from(glyphs.height()) * 0.2;
    let central_y1 = f64::from(glyphs.y1) - f64::from(glyphs.height()) * 0.2;
    let central_x0 = f64::from(glyphs.x0) + f64::from(glyphs.width()) * 0.2;
    let central_x1 = f64::from(glyphs.x1) - f64::from(glyphs.width()) * 0.2;
    let horizontal = f64::from(span_x) >= f64::from(glyphs.width()) * ratio
        && f64::from(max_y) >= central_y0
        && f64::from(min_y) <= central_y1;
    let vertical = f64::from(span_y) >= f64::from(glyphs.height()) * ratio
        && f64::from(max_x) >= central_x0
        && f64::from(min_x) <= central_x1;
    horizontal || vertical
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
            let value = argument
                .to_str()
                .ok_or_else(|| GuardError(format!("{option} value is not valid UTF-8")))?;
            match option.as_str() {
                "--scale" => config.scale = parse_number(value, &option)?,
                "--min-contrast" => config.min_contrast = parse_number(value, &option)?,
                "--crossing-ratio" => config.crossing_ratio = parse_number(value, &option)?,
                "--font-dir" => config.font_dirs.push(PathBuf::from(argument)),
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
OPTIONS\n  --json                 Emit a machine-readable report\n  --scale NUMBER         Fixed rasterization scale [default: 2]\n  --min-contrast NUMBER  Minimum text/background contrast [default: 4.5]\n  --crossing-ratio N     Fraction of a text block a stroke must traverse [default: 0.45]\n  --font-dir DIR         Add a font directory; repeatable\n  -h, --help             Show this help\n  -V, --version          Show the version\n\n\
Warnings are advisory and return exit code 0. Analysis errors return exit code 2."
    );
}
