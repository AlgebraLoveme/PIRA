use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use std::collections::HashSet;

use crate::GuardError;

const KEY_ATTRIBUTE: &[u8] = b"data-pira-svg-check-key";
const MAX_NODES: usize = 50_000;

#[derive(Clone, Debug)]
pub(crate) struct ElementMeta {
    pub key: String,
    pub id: Option<String>,
    pub tag: String,
    pub text: String,
    pub clipped_or_masked: bool,
}

impl ElementMeta {
    pub fn label(&self) -> String {
        match &self.id {
            Some(id) => format!("<{}#{}>", self.tag, id),
            None => format!("<{}@{}>", self.tag, self.key),
        }
    }
}

#[derive(Debug)]
pub(crate) struct AnnotatedSvg {
    pub source: Vec<u8>,
    pub texts: Vec<ElementMeta>,
    pub candidates: Vec<ElementMeta>,
}

#[derive(Clone, Copy)]
pub(crate) enum Variant {
    IsolateText,
    IsolateStroke,
    Remove,
}

struct Frame {
    name: String,
    started_target: bool,
    text_index: Option<usize>,
    self_clipped: bool,
}

pub(crate) fn annotate(source: &[u8]) -> Result<AnnotatedSvg, GuardError> {
    validate_source(source)?;
    let mut reader = Reader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(source.len() + 1024));
    let mut buffer = Vec::new();
    let mut texts = Vec::new();
    let mut candidates = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut defs_depth = 0_usize;
    let mut text_depth = 0_usize;
    let mut clip_depth = 0_usize;
    let mut node_count = 0_usize;
    let mut keys = HashSet::new();

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| GuardError(format!("invalid SVG XML: {error}")))?;
        match event {
            Event::Start(element) => {
                node_count += 1;
                check_node_limit(node_count)?;
                let name = local_name(element.name().as_ref())?;
                let inside_defs = defs_depth > 0 || is_definition_container(&name);
                validate_element_resources(&element, &reader)?;
                let self_clipped = has_clip_or_mask(&element, &reader)?;
                let clipped = clip_depth > 0 || self_clipped;
                let target = target_kind(&name, inside_defs, text_depth);
                let (rewritten, meta) =
                    annotate_element(element, target, node_count, clipped, &reader)?;
                let text_index = if let Some(meta) = meta {
                    if !keys.insert(meta.key.clone()) {
                        return Err(GuardError(
                            "duplicate internal PIRA SVG check key".to_string(),
                        ));
                    }
                    if name == "text" {
                        texts.push(meta);
                        Some(texts.len() - 1)
                    } else {
                        candidates.push(meta);
                        None
                    }
                } else {
                    None
                };
                writer
                    .write_event(Event::Start(rewritten))
                    .map_err(write_error)?;
                stack.push(Frame {
                    name: name.clone(),
                    started_target: false,
                    text_index,
                    self_clipped,
                });
                if is_definition_container(&name) {
                    defs_depth += 1;
                }
                if name == "text" {
                    text_depth += 1;
                }
                if self_clipped {
                    clip_depth += 1;
                }
            }
            Event::Empty(element) => {
                node_count += 1;
                check_node_limit(node_count)?;
                let name = local_name(element.name().as_ref())?;
                let inside_defs = defs_depth > 0 || is_definition_container(&name);
                validate_element_resources(&element, &reader)?;
                let clipped = clip_depth > 0 || has_clip_or_mask(&element, &reader)?;
                let target = target_kind(&name, inside_defs, text_depth);
                let (rewritten, meta) =
                    annotate_element(element, target, node_count, clipped, &reader)?;
                if let Some(meta) = meta {
                    if name == "text" {
                        texts.push(meta);
                    } else {
                        candidates.push(meta);
                    }
                }
                writer
                    .write_event(Event::Empty(rewritten))
                    .map_err(write_error)?;
            }
            Event::Text(text) => {
                if let Some(index) = stack.iter().rev().find_map(|frame| frame.text_index) {
                    texts[index]
                        .text
                        .push_str(&String::from_utf8_lossy(text.as_ref()));
                }
                writer
                    .write_event(Event::Text(text.into_owned()))
                    .map_err(write_error)?;
            }
            Event::CData(text) => {
                if let Some(index) = stack.iter().rev().find_map(|frame| frame.text_index) {
                    texts[index]
                        .text
                        .push_str(&String::from_utf8_lossy(text.as_ref()));
                }
                writer
                    .write_event(Event::CData(text.into_owned()))
                    .map_err(write_error)?;
            }
            Event::End(end) => {
                let frame = stack
                    .pop()
                    .ok_or_else(|| GuardError("malformed SVG element stack".to_string()))?;
                if frame.name == "text" {
                    text_depth = text_depth.saturating_sub(1);
                }
                if is_definition_container(&frame.name) {
                    defs_depth = defs_depth.saturating_sub(1);
                }
                if frame.self_clipped {
                    clip_depth = clip_depth.saturating_sub(1);
                }
                writer
                    .write_event(Event::End(end.into_owned()))
                    .map_err(write_error)?;
            }
            Event::Eof => break,
            other => writer
                .write_event(other.into_owned())
                .map_err(write_error)?,
        }
        buffer.clear();
    }

    for meta in &mut texts {
        meta.text = normalize_text(&meta.text);
    }
    texts.retain(|meta| !meta.text.is_empty());
    Ok(AnnotatedSvg {
        source: writer.into_inner(),
        texts,
        candidates,
    })
}

pub(crate) fn rewrite(
    source: &[u8],
    target_key: &str,
    variant: Variant,
) -> Result<Vec<u8>, GuardError> {
    let mut reader = Reader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(source.len() + 1024));
    let mut buffer = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut defs_depth = 0_usize;
    let mut target_depth = 0_usize;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| GuardError(format!("cannot rewrite SVG variant: {error}")))?;
        match event {
            Event::Start(element) => {
                let name = local_name(element.name().as_ref())?;
                let is_target = attribute_value(&element, KEY_ATTRIBUTE, &reader)?.as_deref()
                    == Some(target_key);
                let inside_target = target_depth > 0 || is_target;
                let inside_defs = defs_depth > 0 || is_definition_container(&name);
                let style = variant_style(&name, inside_defs, inside_target, is_target, variant);
                let rewritten = with_style(element, style.as_deref(), &reader)?;
                writer
                    .write_event(Event::Start(rewritten))
                    .map_err(write_error)?;
                stack.push(Frame {
                    name: name.clone(),
                    started_target: is_target,
                    text_index: None,
                    self_clipped: false,
                });
                if is_definition_container(&name) {
                    defs_depth += 1;
                }
                if is_target {
                    target_depth += 1;
                }
            }
            Event::Empty(element) => {
                let name = local_name(element.name().as_ref())?;
                let is_target = attribute_value(&element, KEY_ATTRIBUTE, &reader)?.as_deref()
                    == Some(target_key);
                let inside_defs = defs_depth > 0 || is_definition_container(&name);
                let style = variant_style(&name, inside_defs, is_target, is_target, variant);
                let rewritten = with_style(element, style.as_deref(), &reader)?;
                writer
                    .write_event(Event::Empty(rewritten))
                    .map_err(write_error)?;
            }
            Event::End(end) => {
                let frame = stack
                    .pop()
                    .ok_or_else(|| GuardError("malformed SVG element stack".to_string()))?;
                if frame.started_target {
                    target_depth = target_depth.saturating_sub(1);
                }
                if is_definition_container(&frame.name) {
                    defs_depth = defs_depth.saturating_sub(1);
                }
                writer
                    .write_event(Event::End(end.into_owned()))
                    .map_err(write_error)?;
            }
            Event::Eof => break,
            other => writer
                .write_event(other.into_owned())
                .map_err(write_error)?,
        }
        buffer.clear();
    }
    Ok(writer.into_inner())
}

fn annotate_element<'a>(
    element: BytesStart<'a>,
    target: bool,
    index: usize,
    clipped: bool,
    reader: &Reader<&[u8]>,
) -> Result<(BytesStart<'static>, Option<ElementMeta>), GuardError> {
    let name = local_name(element.name().as_ref())?;
    let id = attribute_value(&element, b"id", reader)?;
    let key = format!("n{index}");
    let mut rewritten = copy_start(&element, None, reader)?;
    if target {
        rewritten.push_attribute((std::str::from_utf8(KEY_ATTRIBUTE).unwrap(), key.as_str()));
    }
    let meta = target.then_some(ElementMeta {
        key,
        id,
        tag: name,
        text: String::new(),
        clipped_or_masked: clipped,
    });
    Ok((rewritten, meta))
}

fn with_style(
    element: BytesStart<'_>,
    addition: Option<&str>,
    reader: &Reader<&[u8]>,
) -> Result<BytesStart<'static>, GuardError> {
    copy_start(&element, addition, reader)
}

fn copy_start(
    element: &BytesStart<'_>,
    style_addition: Option<&str>,
    reader: &Reader<&[u8]>,
) -> Result<BytesStart<'static>, GuardError> {
    let qualified_name = String::from_utf8_lossy(element.name().as_ref()).into_owned();
    let mut rewritten = BytesStart::new(qualified_name);
    let mut style = None;
    for attribute in element.attributes().with_checks(false) {
        let attribute =
            attribute.map_err(|error| GuardError(format!("invalid SVG attribute: {error}")))?;
        if attribute.key.local_name().as_ref() == b"style" {
            style = Some(
                attribute
                    .decode_and_unescape_value(reader.decoder())
                    .map_err(|error| GuardError(format!("invalid SVG style: {error}")))?
                    .into_owned(),
            );
        } else {
            rewritten.push_attribute(attribute.to_owned());
        }
    }
    if let Some(addition) = style_addition {
        let combined = match style {
            Some(existing) if !existing.trim().is_empty() => {
                format!("{};{}", existing.trim_end_matches(';'), addition)
            }
            _ => addition.to_string(),
        };
        rewritten.push_attribute(("style", combined.as_str()));
    } else if let Some(style) = style {
        rewritten.push_attribute(("style", style.as_str()));
    }
    Ok(rewritten.into_owned())
}

fn variant_style(
    name: &str,
    inside_defs: bool,
    inside_target: bool,
    is_target: bool,
    variant: Variant,
) -> Option<String> {
    match variant {
        Variant::Remove if is_target => Some("display:none!important".to_string()),
        Variant::Remove => None,
        Variant::IsolateText => {
            if !inside_defs && is_paintable(name) && !inside_target {
                Some("display:none!important".to_string())
            } else if inside_target && matches!(name, "text" | "tspan") {
                Some("stroke:none!important;filter:none!important".to_string())
            } else {
                None
            }
        }
        Variant::IsolateStroke => {
            if !inside_defs && is_paintable(name) && !inside_target {
                Some("display:none!important".to_string())
            } else if inside_target {
                Some("fill:none!important;filter:none!important".to_string())
            } else {
                None
            }
        }
    }
}

fn target_kind(name: &str, inside_defs: bool, text_depth: usize) -> bool {
    if inside_defs {
        return false;
    }
    name == "text" || (text_depth == 0 && is_stroke_candidate(name))
}

fn is_paintable(name: &str) -> bool {
    matches!(
        name,
        "circle"
            | "ellipse"
            | "image"
            | "line"
            | "path"
            | "polygon"
            | "polyline"
            | "rect"
            | "text"
            | "tspan"
            | "use"
    )
}

fn is_stroke_candidate(name: &str) -> bool {
    matches!(
        name,
        "circle" | "ellipse" | "line" | "path" | "polygon" | "polyline" | "rect" | "use"
    )
}

fn is_definition_container(name: &str) -> bool {
    matches!(
        name,
        "defs" | "symbol" | "clipPath" | "mask" | "pattern" | "marker"
    )
}

fn has_clip_or_mask(element: &BytesStart<'_>, reader: &Reader<&[u8]>) -> Result<bool, GuardError> {
    for attribute in element.attributes().with_checks(false) {
        let attribute =
            attribute.map_err(|error| GuardError(format!("invalid SVG attribute: {error}")))?;
        let name = attribute.key.local_name();
        if matches!(name.as_ref(), b"clip-path" | b"mask") {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| GuardError(format!("invalid SVG attribute value: {error}")))?;
            if value.trim() != "none" {
                return Ok(true);
            }
        }
        if name.as_ref() == b"style" {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| GuardError(format!("invalid SVG style: {error}")))?;
            let compact = value.to_ascii_lowercase().replace(' ', "");
            if (compact.contains("clip-path:") && !compact.contains("clip-path:none"))
                || (compact.contains("mask:") && !compact.contains("mask:none"))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn validate_element_resources(
    element: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
) -> Result<(), GuardError> {
    for attribute in element.attributes().with_checks(false) {
        let attribute =
            attribute.map_err(|error| GuardError(format!("invalid SVG attribute: {error}")))?;
        if matches!(attribute.key.local_name().as_ref(), b"href" | b"src") {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| GuardError(format!("invalid SVG resource reference: {error}")))?;
            let value = value.trim();
            if !value.is_empty() && !value.starts_with('#') && !value.starts_with("data:") {
                return Err(GuardError(format!(
                    "external SVG resource is not allowed: {value}"
                )));
            }
        }
        if attribute.key.as_ref() == KEY_ATTRIBUTE {
            return Err(GuardError(format!(
                "reserved SVG attribute is not allowed: {}",
                String::from_utf8_lossy(KEY_ATTRIBUTE)
            )));
        }
    }
    Ok(())
}

fn attribute_value(
    element: &BytesStart<'_>,
    name: &[u8],
    reader: &Reader<&[u8]>,
) -> Result<Option<String>, GuardError> {
    for attribute in element.attributes().with_checks(false) {
        let attribute =
            attribute.map_err(|error| GuardError(format!("invalid SVG attribute: {error}")))?;
        if attribute.key.local_name().as_ref() == name {
            return attribute
                .decode_and_unescape_value(reader.decoder())
                .map(|value| Some(value.into_owned()))
                .map_err(|error| GuardError(format!("invalid SVG attribute value: {error}")));
        }
    }
    Ok(None)
}

fn validate_source(source: &[u8]) -> Result<(), GuardError> {
    let lowered = String::from_utf8_lossy(source).to_ascii_lowercase();
    for forbidden in ["<!doctype", "<!entity", "<?xml-stylesheet", "@import"] {
        if lowered.contains(forbidden) {
            return Err(GuardError(format!(
                "unsafe or external SVG construct is not allowed: {forbidden}"
            )));
        }
    }
    let mut remainder = lowered.as_str();
    while let Some(position) = remainder.find("url(") {
        remainder = &remainder[position + 4..];
        let Some(end) = remainder.find(')') else {
            break;
        };
        let target = remainder[..end].trim().trim_matches(['\'', '"']);
        if !target.starts_with('#') && !target.starts_with("data:") {
            return Err(GuardError(format!(
                "external SVG resource is not allowed: {target}"
            )));
        }
        remainder = &remainder[end + 1..];
    }
    Ok(())
}

fn local_name(name: &[u8]) -> Result<String, GuardError> {
    let name = std::str::from_utf8(name)
        .map_err(|_| GuardError("SVG element name is not valid UTF-8".to_string()))?;
    Ok(name.rsplit(':').next().unwrap_or(name).to_string())
}

fn normalize_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn check_node_limit(count: usize) -> Result<(), GuardError> {
    if count > MAX_NODES {
        Err(GuardError(
            "SVG exceeds the element-count limit".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn write_error(error: std::io::Error) -> GuardError {
    GuardError(format!("cannot rewrite SVG: {error}"))
}

#[cfg(test)]
mod tests {
    use super::annotate;

    #[test]
    fn clip_state_survives_nested_and_sibling_elements() {
        let source = br#"<svg xmlns="http://www.w3.org/2000/svg">
            <g clip-path="url(#clip)"><g><text>A</text></g><text>B</text></g>
        </svg>"#;
        let annotated = annotate(source).expect("SVG should annotate");
        assert_eq!(annotated.texts.len(), 2);
        assert!(annotated.texts.iter().all(|text| text.clipped_or_masked));
    }

    #[test]
    fn rejects_external_href() {
        let source = br#"<svg xmlns="http://www.w3.org/2000/svg">
            <image href="https://example.com/untrusted.png"/>
        </svg>"#;
        let error = annotate(source).expect_err("external resources must fail");
        assert!(error.to_string().contains("external SVG resource"));
    }
}
