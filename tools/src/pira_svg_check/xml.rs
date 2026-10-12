use quick_xml::events::{BytesStart, Event};
use quick_xml::{
    NsReader, Writer,
    name::{QName, ResolveResult},
};
use std::collections::HashSet;

use crate::{GuardError, css};

const KEY_ATTRIBUTE: &[u8] = b"data-pira-svg-check-key";
const MAX_NODES: usize = 50_000;
const SVG_NS: &[u8] = b"http://www.w3.org/2000/svg";
const XML_NS: &[u8] = b"http://www.w3.org/XML/1998/namespace";
const XLINK_NS: &[u8] = b"http://www.w3.org/1999/xlink";

#[derive(Clone, Debug)]
pub(crate) struct ElementMeta {
    pub key: String,
    pub id: Option<String>,
    pub tag: String,
    pub text: String,
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
    TextCoverage,
    IsolateStroke,
    Remove,
    Unclip,
}

struct Frame {
    name: String,
    started_target: bool,
    text_index: Option<usize>,
}

// PIRA: conservative recursion ceiling before entering usvg. Supporting deeper
// artwork requires a renderer with bounded/iterative traversal, not a larger cap.
pub(crate) const MAX_RENDER_DEPTH: usize = 64;

pub(crate) fn render_depth(source: &[u8], parent_depth: usize) -> Result<usize, GuardError> {
    if source.starts_with(&[0x1f, 0x8b]) {
        return Err(GuardError(
            "compressed SVG is unsupported for bounded scene analysis".into(),
        ));
    }
    let mut reader = quick_xml::Reader::from_reader(source);
    let mut depth = parent_depth;
    let mut maximum = depth;
    let mut nodes = 0;
    loop {
        match reader
            .read_event()
            .map_err(|error| GuardError(format!("invalid SVG XML: {error}")))?
        {
            Event::Start(_) => {
                depth += 1;
                maximum = maximum.max(depth);
                nodes += 1;
            }
            Event::Empty(_) => {
                maximum = maximum.max(depth + 1);
                nodes += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::DocType(_) => {
                return Err(GuardError("SVG document types are not allowed".into()));
            }
            Event::Eof => break,
            _ => {}
        }
        if maximum > MAX_RENDER_DEPTH {
            return Err(GuardError(format!(
                "SVG exceeds the render nesting limit ({MAX_RENDER_DEPTH}); simplify nested artwork"
            )));
        }
        check_node_limit(nodes)?;
    }
    Ok(maximum)
}

// Resource policy applies to every parsed document, independently of outer-only
// annotation keys and text discovery beneath foreign ancestors.
pub(crate) fn validate_resources(source: &[u8]) -> Result<(), GuardError> {
    let mut reader = NsReader::from_reader(source);
    let mut styles: Vec<Option<String>> = Vec::new();
    let mut nodes = 0;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| GuardError(format!("invalid SVG XML: {error}")))?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                nodes += 1;
                check_node_limit(nodes)?;
                let name = svg_name(element.name(), &reader)?;
                validate_element_resources(element, &reader, !name.is_empty())?;
                if matches!(event, Event::Start(_)) {
                    styles.push((name == "style").then(String::new));
                }
            }
            Event::Text(text) => {
                if let Some(css) = styles.last_mut().and_then(Option::as_mut) {
                    css.push_str(&String::from_utf8_lossy(text.as_ref()));
                }
            }
            Event::CData(text) => {
                if let Some(css) = styles.last_mut().and_then(Option::as_mut) {
                    css.push_str(&String::from_utf8_lossy(text.as_ref()));
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(css) = styles.last_mut().and_then(Option::as_mut) {
                    let encoded = format!("&{};", String::from_utf8_lossy(reference.as_ref()));
                    let decoded = quick_xml::escape::unescape(&encoded).map_err(|error| {
                        GuardError(format!("invalid SVG entity reference: {error}"))
                    })?;
                    css.push_str(&decoded);
                }
            }
            Event::End(_) => {
                if let Some(Some(css)) = styles.pop() {
                    css::validate_urls(&css)?;
                }
            }
            Event::DocType(_) => {
                return Err(GuardError("SVG document types are not allowed".to_string()));
            }
            Event::PI(ref instruction)
                if instruction.target().eq_ignore_ascii_case(b"xml-stylesheet") =>
            {
                return Err(GuardError(
                    "external SVG stylesheets are not allowed".to_string(),
                ));
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

pub(crate) fn annotate(source: &[u8]) -> Result<AnnotatedSvg, GuardError> {
    validate_resources(source)?;
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(source.len() + 1024));
    let mut buffer = Vec::new();
    let mut texts = Vec::new();
    let mut candidates = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut defs_depth = 0_usize;
    let mut text_depth = 0_usize;
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
                let mut name = svg_name(element.name(), &reader)?;
                if stack.last().is_some_and(|frame| frame.name.is_empty()) {
                    name.clear();
                }
                let inside_defs = defs_depth > 0 || is_definition_container(&name);
                let target = target_kind(&name, inside_defs, text_depth);
                let (rewritten, meta) = annotate_element(element, target, node_count, &reader)?;
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
                });
                if is_definition_container(&name) {
                    defs_depth += 1;
                }
                if name == "text" {
                    text_depth += 1;
                }
            }
            Event::Empty(element) => {
                node_count += 1;
                check_node_limit(node_count)?;
                let mut name = svg_name(element.name(), &reader)?;
                if stack.last().is_some_and(|frame| frame.name.is_empty()) {
                    name.clear();
                }
                let inside_defs = defs_depth > 0 || is_definition_container(&name);
                let target = target_kind(&name, inside_defs, text_depth);
                let (rewritten, meta) = annotate_element(element, target, node_count, &reader)?;
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
                if let Some(index) = stack
                    .iter()
                    .rev()
                    .take_while(|frame| !frame.name.is_empty())
                    .find_map(|frame| frame.text_index)
                {
                    texts[index]
                        .text
                        .push_str(&String::from_utf8_lossy(text.as_ref()));
                }
                writer
                    .write_event(Event::Text(text.into_owned()))
                    .map_err(write_error)?;
            }
            Event::CData(text) => {
                if let Some(index) = stack
                    .iter()
                    .rev()
                    .take_while(|frame| !frame.name.is_empty())
                    .find_map(|frame| frame.text_index)
                {
                    texts[index]
                        .text
                        .push_str(&String::from_utf8_lossy(text.as_ref()));
                }
                writer
                    .write_event(Event::CData(text.into_owned()))
                    .map_err(write_error)?;
            }
            Event::GeneralRef(reference) => {
                let encoded = format!("&{};", String::from_utf8_lossy(reference.as_ref()));
                let decoded = quick_xml::escape::unescape(&encoded).map_err(|error| {
                    GuardError(format!("invalid SVG entity reference: {error}"))
                })?;
                if let Some(index) = stack
                    .iter()
                    .rev()
                    .take_while(|frame| !frame.name.is_empty())
                    .find_map(|frame| frame.text_index)
                {
                    texts[index].text.push_str(&decoded);
                }
                writer
                    .write_event(Event::GeneralRef(reference.into_owned()))
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
    let target_path = if matches!(variant, Variant::IsolateText | Variant::IsolateStroke) {
        target_path(source, target_key)?
    } else {
        HashSet::new()
    };
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(source.len() + 1024));
    let mut buffer = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut defs_depth = 0_usize;
    let mut target_depth = 0_usize;
    let mut node_count = 0;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| GuardError(format!("cannot rewrite SVG variant: {error}")))?;
        match event {
            Event::Start(element) => {
                node_count += 1;
                let mut name = svg_name(element.name(), &reader)?;
                if stack.last().is_some_and(|frame| frame.name.is_empty()) {
                    name.clear();
                }
                let is_target = attribute_value(&element, KEY_ATTRIBUTE, &reader)?.as_deref()
                    == Some(target_key);
                let inside_target = target_depth > 0 || is_target;
                let inside_defs = defs_depth > 0 || is_definition_container(&name);
                let style = variant_style(
                    &name,
                    inside_defs,
                    inside_target,
                    is_target,
                    target_path.contains(&node_count),
                    variant,
                );
                let rewritten = with_style(element, style.as_deref(), &reader)?;
                writer
                    .write_event(Event::Start(rewritten))
                    .map_err(write_error)?;
                stack.push(Frame {
                    name: name.clone(),
                    started_target: is_target,
                    text_index: None,
                });
                if is_definition_container(&name) {
                    defs_depth += 1;
                }
                if is_target {
                    target_depth += 1;
                }
            }
            Event::Empty(element) => {
                node_count += 1;
                let mut name = svg_name(element.name(), &reader)?;
                if stack.last().is_some_and(|frame| frame.name.is_empty()) {
                    name.clear();
                }
                let is_target = attribute_value(&element, KEY_ATTRIBUTE, &reader)?.as_deref()
                    == Some(target_key);
                let inside_defs = defs_depth > 0 || is_definition_container(&name);
                let inside_target = target_depth > 0 || is_target;
                let style = variant_style(
                    &name,
                    inside_defs,
                    inside_target,
                    is_target,
                    target_path.contains(&node_count),
                    variant,
                );
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

// Hiding only leaves does not suppress a sibling container's generated filter
// output. Retain target ancestors, but hide unrelated scene containers as units.
fn target_path(source: &[u8], target_key: &str) -> Result<HashSet<usize>, GuardError> {
    let mut reader = NsReader::from_reader(source);
    let mut path = Vec::new();
    let mut nodes = 0;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| GuardError(format!("cannot rewrite SVG variant: {error}")))?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                nodes += 1;
                if attribute_value(element, KEY_ATTRIBUTE, &reader)?.as_deref() == Some(target_key)
                {
                    path.push(nodes);
                    return Ok(path.into_iter().collect());
                }
                if matches!(event, Event::Start(_)) {
                    path.push(nodes);
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => return Ok(HashSet::new()),
            _ => {}
        }
    }
}

// usvg recognizes some foreign attributes by local name. Only pass attributes
// with SVG/XML/XLink meaning to that boundary; retain original metadata elsewhere.
pub(crate) fn renderer_source(source: &[u8]) -> Result<Vec<u8>, GuardError> {
    let mut reader = NsReader::from_reader(source);
    let mut writer = Writer::new(Vec::with_capacity(source.len()));
    let mut active = Vec::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|error| GuardError(format!("invalid SVG XML: {error}")))?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let is_svg = !svg_name(element.name(), &reader)?.is_empty()
                    && active.last().copied().unwrap_or(true);
                if matches!(event, Event::Start(_)) {
                    active.push(is_svg);
                }
                let mut clean =
                    BytesStart::new(String::from_utf8_lossy(element.name().as_ref()).into_owned());
                for attribute in element.attributes() {
                    let attribute = attribute
                        .map_err(|error| GuardError(format!("invalid SVG attribute: {error}")))?;
                    let keep = if attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:")
                    {
                        true
                    } else {
                        meaningful_attribute(attribute.key, &reader)?
                    };
                    if keep {
                        clean.push_attribute(attribute);
                    }
                }
                writer
                    .write_event(if matches!(event, Event::Empty(_)) {
                        Event::Empty(clean)
                    } else {
                        Event::Start(clean)
                    })
                    .map_err(write_error)?;
            }
            // usvg scans every local-name style element, even in foreign
            // namespaces. Preserve the XML structure, but not foreign content
            // that it could incorrectly treat as a stylesheet.
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_)
                if active.last() == Some(&false) => {}
            Event::End(end) => {
                active.pop();
                writer.write_event(Event::End(end)).map_err(write_error)?;
            }
            Event::Eof => break,
            other => writer.write_event(other).map_err(write_error)?,
        }
    }
    Ok(writer.into_inner())
}

fn annotate_element<'a>(
    element: BytesStart<'a>,
    target: bool,
    index: usize,
    reader: &NsReader<&[u8]>,
) -> Result<(BytesStart<'static>, Option<ElementMeta>), GuardError> {
    if attribute_value(&element, KEY_ATTRIBUTE, reader)?.is_some() {
        return Err(GuardError(format!(
            "reserved SVG attribute is not allowed: {}",
            String::from_utf8_lossy(KEY_ATTRIBUTE)
        )));
    }
    let name = svg_name(element.name(), reader)?;
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
    });
    Ok((rewritten, meta))
}

fn with_style(
    element: BytesStart<'_>,
    addition: Option<&str>,
    reader: &NsReader<&[u8]>,
) -> Result<BytesStart<'static>, GuardError> {
    copy_start(&element, addition, reader)
}

fn copy_start(
    element: &BytesStart<'_>,
    style_addition: Option<&str>,
    reader: &NsReader<&[u8]>,
) -> Result<BytesStart<'static>, GuardError> {
    let qualified_name = String::from_utf8_lossy(element.name().as_ref()).into_owned();
    let mut rewritten = BytesStart::new(qualified_name);
    let mut style = None;
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| GuardError(format!("invalid SVG attribute: {error}")))?;
        if attribute.key.as_ref() == b"style" {
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
        // Author rules have been flattened into inline declarations. usvg keeps
        // the first important declaration, so exact-node overrides go first.
        let combined = match style {
            Some(existing) if !existing.trim().is_empty() => {
                format!("{};{}", addition, existing)
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
    on_target_path: bool,
    variant: Variant,
) -> Option<String> {
    let hide_sibling = !inside_defs
        && !inside_target
        && (is_paintable(name)
            || (matches!(name, "g" | "svg" | "a" | "switch") && !on_target_path));
    match variant {
        // Only coverage loses paint opacity. Actual composited colors and the
        // loss/not-rendered probes still use the unmodified isolated artwork.
        Variant::TextCoverage if !inside_defs => Some(
            if matches!(name, "text" | "tspan") {
                "opacity:1!important;fill-opacity:1!important"
            } else {
                "opacity:1!important"
            }
            .into(),
        ),
        Variant::TextCoverage => None,
        Variant::Unclip if !inside_defs => {
            Some("clip-path:none!important;mask:none!important".into())
        }
        Variant::Unclip => None,
        Variant::Remove if is_target => Some("display:none!important".to_string()),
        Variant::Remove => None,
        Variant::IsolateText | Variant::IsolateStroke if hide_sibling => {
            Some("display:none!important".to_string())
        }
        Variant::IsolateText if inside_target && matches!(name, "text" | "tspan") => {
            Some("stroke:none!important;filter:none!important".to_string())
        }
        Variant::IsolateStroke if inside_target => {
            Some("fill:none!important;filter:none!important".to_string())
        }
        Variant::IsolateText | Variant::IsolateStroke => None,
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

fn validate_element_resources(
    element: &BytesStart<'_>,
    reader: &NsReader<&[u8]>,
    svg_element: bool,
) -> Result<(), GuardError> {
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| GuardError(format!("invalid SVG attribute: {error}")))?;
        let meaningful = meaningful_attribute(attribute.key, reader)?;
        if svg_element
            && meaningful
            && matches!(attribute.key.local_name().as_ref(), b"href" | b"src")
        {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| GuardError(format!("invalid SVG resource reference: {error}")))?;
            let value = value.trim();
            css::validate_reference(value)?;
        }
        if svg_element
            && matches!(
                attribute.key.as_ref(),
                b"style"
                    | b"fill"
                    | b"stroke"
                    | b"filter"
                    | b"clip-path"
                    | b"mask"
                    | b"marker"
                    | b"marker-start"
                    | b"marker-mid"
                    | b"marker-end"
                    | b"cursor"
                    | b"color-profile"
                    | b"background"
                    | b"background-image"
                    | b"list-style"
                    | b"list-style-image"
            )
        {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| GuardError(format!("invalid SVG CSS value: {error}")))?;
            css::validate_urls(&value)?;
        }
    }
    Ok(())
}

fn attribute_value(
    element: &BytesStart<'_>,
    name: &[u8],
    reader: &NsReader<&[u8]>,
) -> Result<Option<String>, GuardError> {
    for attribute in element.attributes().with_checks(false) {
        let attribute =
            attribute.map_err(|error| GuardError(format!("invalid SVG attribute: {error}")))?;
        if attribute.key.as_ref() == name {
            return attribute
                .decode_and_unescape_value(reader.decoder())
                .map(|value| Some(value.into_owned()))
                .map_err(|error| GuardError(format!("invalid SVG attribute value: {error}")));
        }
    }
    Ok(None)
}

fn svg_name(name: QName<'_>, reader: &NsReader<&[u8]>) -> Result<String, GuardError> {
    let (namespace, local) = reader.resolve_element(name);
    match namespace {
        ResolveResult::Unbound => {}
        ResolveResult::Bound(namespace) if namespace.as_ref() == SVG_NS => {}
        ResolveResult::Bound(_) => return Ok(String::new()),
        ResolveResult::Unknown(_) => {
            return Err(GuardError("unbound SVG element namespace".to_string()));
        }
    }
    std::str::from_utf8(local.as_ref())
        .map(str::to_owned)
        .map_err(|_| GuardError("SVG element name is not valid UTF-8".to_string()))
}

fn meaningful_attribute(name: QName<'_>, reader: &NsReader<&[u8]>) -> Result<bool, GuardError> {
    if name.as_ref() == b"xmlns" || name.as_ref().starts_with(b"xmlns:") {
        return Ok(false);
    }
    let (namespace, local) = reader.resolve_attribute(name);
    match namespace {
        ResolveResult::Unbound => Ok(true),
        ResolveResult::Bound(namespace) => Ok((namespace.as_ref() == XML_NS
            && matches!(local.as_ref(), b"space" | b"lang" | b"base"))
            || (namespace.as_ref() == XLINK_NS && local.as_ref() == b"href")),
        ResolveResult::Unknown(_) => Err(GuardError("unbound SVG attribute namespace".to_string())),
    }
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

// Local references are resolved by usvg before variants can hide source nodes.
pub(crate) fn has_local_references(source: &[u8]) -> Result<bool, GuardError> {
    let mut reader = NsReader::from_reader(source);
    loop {
        match reader
            .read_event()
            .map_err(|error| GuardError(error.to_string()))?
        {
            Event::Start(ref element) | Event::Empty(ref element) => {
                if svg_name(element.name(), &reader)?.is_empty() {
                    continue;
                }
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| GuardError(error.to_string()))?;
                    if meaningful_attribute(attribute.key, &reader)?
                        && attribute.key.local_name().as_ref() == b"href"
                    {
                        let value = attribute
                            .decode_and_unescape_value(reader.decoder())
                            .map_err(|error| GuardError(error.to_string()))?;
                        if value.trim().starts_with('#') {
                            return Ok(true);
                        }
                    }
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Variant, annotate, rewrite};

    #[test]
    fn foreign_ancestors_do_not_exempt_genuine_svg_resources() {
        for resource in [
            "<image href='https://example.invalid/image.png'/>",
            "<image href='https://example.invalid/image.png'></image>",
            "<s:image l:href='https://example.invalid/image.png'/>",
            "<use href='https://example.invalid/figure.svg#shape'/>",
            "<feImage l:href='https://example.invalid/image.png'/>",
            "<s:rect fill='u&#114;l(https://example.invalid/paint)'/>",
            "<rect style='filter:url(https://example.invalid/filter)'/>",
            "<s:style>text {fill:url(https://example.invalid/paint)}</s:style>",
            "<style><![CDATA[@import 'https://example.invalid/style';]]></style>",
        ] {
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' xmlns:s='http://www.w3.org/2000/svg' xmlns:m='urn:meta' xmlns:l='http://www.w3.org/1999/xlink'><m:container><m:nested>{resource}</m:nested></m:container></svg>"
            );
            // Annotation is the input boundary, before any renderer/file access.
            assert!(
                annotate(source.as_bytes())
                    .unwrap_err()
                    .to_string()
                    .contains("external"),
                "{resource}"
            );
        }
        for resource in [
            "<m:image href='https://example.invalid/metadata'/>",
            "<m:style>@import 'https://example.invalid/metadata';</m:style>",
            "<s:image m:href='https://example.invalid/metadata' href='data:image/png;base64,AAAA'/>",
            "<s:use l:href='#local'/>",
            "<s:rect fill='url(#local)'/>",
        ] {
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' xmlns:s='http://www.w3.org/2000/svg' xmlns:m='urn:meta' xmlns:l='http://www.w3.org/1999/xlink'><m:container>{resource}</m:container></svg>"
            );
            assert!(annotate(source.as_bytes()).is_ok(), "{resource}");
        }
    }

    #[test]
    fn namespaces_distinguish_metadata_from_svg_resources() {
        let source = br##"<s:svg xmlns:s="http://www.w3.org/2000/svg" xmlns:m="urn:meta" xmlns:l="http://www.w3.org/1999/xlink">
          <m:text>metadata</m:text><m:container><s:text>also metadata</s:text></m:container>
          <s:text id="real" m:href="https://example.invalid">A<m:text>ignored</m:text>B</s:text>
          <s:use l:href="#real"/>
        </s:svg>"##;
        let annotated = annotate(source).unwrap();
        assert_eq!(annotated.texts.len(), 1);
        assert_eq!(annotated.texts[0].text, "AB");
        assert_eq!(annotated.texts[0].id.as_deref(), Some("real"));
        for attrs in [
            "href='https://example.invalid'",
            "xmlns:l='http://www.w3.org/1999/xlink' l:href='https://example.invalid'",
            "xmlns:l='urn:meta' xmlns:q='http://www.w3.org/1999/xlink' q:href='https://example.invalid'",
            "style='fill:url(https://example.invalid)'",
        ] {
            let source =
                format!("<s:svg xmlns:s='http://www.w3.org/2000/svg'><s:use {attrs}/></s:svg>");
            assert!(
                annotate(source.as_bytes())
                    .unwrap_err()
                    .to_string()
                    .contains("external"),
                "{attrs}"
            );
        }
        assert!(annotate(b"<svg><q:text>unknown</q:text></svg>").is_err());
    }

    #[test]
    fn renderer_boundary_resolves_scoped_namespaces_and_preserves_original() {
        let source = br##"<svg xmlns:m="urn:metadata" xmlns:l="http://www.w3.org/1999/xlink"><text m:style="fill:none" xml:space="preserve">Hi</text><use l:href="#label"/><g xmlns:l="urn:metadata"><text l:style="fill:none"/></g></svg>"##;
        let annotated = annotate(source).unwrap();
        let rendered =
            String::from_utf8(super::renderer_source(&annotated.source).unwrap()).unwrap();
        assert!(!rendered.contains("m:style"));
        assert!(!rendered.contains("l:style"));
        assert!(rendered.contains("xml:space=\"preserve\""));
        assert!(rendered.contains("l:href=\"#label\""));
        assert!(
            String::from_utf8(annotated.source)
                .unwrap()
                .contains("m:style")
        );
        assert!(super::renderer_source(b"<svg m:style='fill:none'/>").is_err());
    }

    #[test]
    fn rejects_external_href() {
        let source = br#"<svg xmlns="http://www.w3.org/2000/svg">
            <image href="https://example.com/untrusted.png"/>
        </svg>"#;
        let error = annotate(source).expect_err("external resources must fail");
        assert!(error.to_string().contains("external SVG resource"));
    }

    #[test]
    fn semantic_text_preserves_entities_and_cdata() {
        for content in [
            "&#67;&#32;&amp;&#32;&#x44;",
            "C &amp; D",
            "<![CDATA[C & D]]>",
        ] {
            let source = format!("<svg><text>{content}</text></svg>");
            let annotated = annotate(source.as_bytes()).unwrap();
            assert_eq!(annotated.texts.len(), 1, "{content}");
            assert_eq!(annotated.texts[0].text, "C & D", "{content}");
            assert!(
                String::from_utf8(annotated.source)
                    .unwrap()
                    .contains(content)
            );
        }
        assert!(annotate(b"<svg><text>&unknown;</text></svg>").is_err());
    }

    #[test]
    fn empty_descendants_keep_target_membership() {
        for child in ["<tspan/>", "<tspan></tspan>"] {
            let source = format!("<svg><text>Hello{child}</text></svg>");
            let annotated = annotate(source.as_bytes()).unwrap();
            let output = rewrite(
                &annotated.source,
                &annotated.texts[0].key,
                Variant::IsolateText,
            )
            .unwrap();
            let output = String::from_utf8(output).unwrap();
            assert!(!output.contains("display:none"), "{output}");
            assert!(
                output.contains("<tspan style=\"stroke:none!important;filter:none!important\"")
            );
        }
    }

    #[test]
    fn duplicate_attributes_are_rejected_before_rewriting() {
        for attrs in [
            r#"style="fill:red" style="fill:black""#,
            r#"fill="red" fill="black""#,
        ] {
            for ending in ["/>", ">Hello</text>"] {
                let source = format!("<svg><text {attrs}{ending}</svg>");
                assert!(
                    annotate(source.as_bytes())
                        .unwrap_err()
                        .to_string()
                        .contains("attribute")
                );
            }
        }
        let annotated = annotate(br#"<svg xmlns:m="urn:metadata"><text m:style="metadata" style="fill:black">Hello</text></svg>"#).unwrap();
        let output = String::from_utf8(annotated.source).unwrap();
        assert!(output.contains("m:style=\"metadata\""));
        assert!(output.contains("style=\"fill:black\""));
    }

    #[test]
    fn encoded_external_css_is_rejected_in_resource_contexts() {
        for content in [
            r#"<text style="fill:u&#114;l(https://example.invalid/a)">Hi</text>"#,
            r#"<text fill="u&#114;l(https://example.invalid/a)">Hi</text>"#,
            "<style>text {fill:u&#114;l(https://example.invalid/a)}</style>",
            r"<style><![CDATA[text {fill:u\72l(https://example.invalid/a)}]]></style>",
            r#"<image href="h&#116;tps://example.invalid/a"/>"#,
        ] {
            let source = format!("<svg>{content}</svg>");
            assert!(
                annotate(source.as_bytes())
                    .unwrap_err()
                    .to_string()
                    .contains("external"),
                "{content}"
            );
        }
    }

    #[test]
    fn local_and_data_css_references_are_preserved() {
        for content in [
            r#"<text fill="url(&quot;#g&quot;)">Hi</text>"#,
            r#"<text style="fill:u&#114;l(&apos;#g&apos;)">Hi</text>"#,
            "<style>text {fill:url(&quot;#g&quot;)}</style>",
            r#"<image href="data:image/png;base64,AAAA"/>"#,
            r#"<text fill="url('data:image/png;base64,AAAA')">Hi</text>"#,
        ] {
            assert!(
                annotate(format!("<svg>{content}</svg>").as_bytes()).is_ok(),
                "{content}"
            );
        }
    }

    #[test]
    fn resource_like_metadata_and_text_are_inert() {
        for content in [
            r#"<text aria-label="url(example)">url(example)</text>"#,
            r#"<text aria-label="u&#114;l(example)">Hi</text>"#,
            "<desc>@import url(example)</desc>",
            "<!-- url(example) --><text>Hi</text>",
            "<text><![CDATA[<!DOCTYPE svg> url(example)]]></text>",
            "<style>text::before {content: 'url(example) @import'}</style>",
        ] {
            assert!(
                annotate(format!("<svg>{content}</svg>").as_bytes()).is_ok(),
                "{content}"
            );
        }
    }

    #[test]
    fn document_types_stylesheet_instructions_and_css_imports_stay_forbidden() {
        for source in [
            "<!DOCTYPE svg><svg/>",
            "<?xml-stylesheet href='external.css'?><svg/>",
            "<svg><style>@import 'external.css';</style></svg>",
            r"<svg><style>@\69mport 'external.css';</style></svg>",
        ] {
            assert!(annotate(source.as_bytes()).is_err(), "{source}");
        }
    }
}
