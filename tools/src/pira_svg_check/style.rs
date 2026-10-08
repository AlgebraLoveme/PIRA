//! Flatten renderer-supported author rules before applying per-node overrides.
use quick_xml::{Reader, Writer, events::Event};
use simplecss::{
    AttributeOperator, Declaration, DeclarationTokenizer, Element, PseudoClass, StyleSheet,
};

use crate::GuardError;

struct Node {
    name: String,
    attributes: Vec<(String, String)>,
    parent: Option<usize>,
    previous: Option<usize>,
    last_child: Option<usize>,
    text: String,
    text_finished: bool,
}

impl Node {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Copy)]
struct NodeRef<'a> {
    nodes: &'a [Node],
    index: usize,
}

// Match usvg's static XmlNode adapter, including local-name attributes and its
// supported pseudo-class. Namespace filtering has already run on this copy.
impl Element for NodeRef<'_> {
    fn parent_element(&self) -> Option<Self> {
        self.nodes[self.index]
            .parent
            .map(|index| Self { index, ..*self })
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.nodes[self.index]
            .previous
            .map(|index| Self { index, ..*self })
    }
    fn has_local_name(&self, name: &str) -> bool {
        self.nodes[self.index].name == name
    }
    fn attribute_matches(&self, name: &str, operator: AttributeOperator<'_>) -> bool {
        self.nodes[self.index]
            .attribute(name)
            .is_some_and(|value| operator.matches(value))
    }
    fn pseudo_class_matches(&self, class: PseudoClass<'_>) -> bool {
        matches!(class, PseudoClass::FirstChild) && self.prev_sibling_element().is_none()
    }
}

fn error(error: impl std::fmt::Display) -> GuardError {
    GuardError(format!("cannot normalize SVG styles: {error}"))
}

pub(super) fn normalize(source: &[u8]) -> Result<Vec<u8>, GuardError> {
    let source = crate::xml::renderer_source(source)?;
    let mut reader = Reader::from_reader(source.as_slice());
    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    loop {
        let event = reader.read_event().map_err(error)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let parent = stack.last().copied();
                let previous = parent.and_then(|parent| nodes[parent].last_child);
                let index = nodes.len();
                if let Some(parent) = parent {
                    nodes[parent].text_finished = true;
                    nodes[parent].last_child = Some(index);
                }
                let mut attributes = Vec::new();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(error)?;
                    if attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:")
                    {
                        continue;
                    }
                    // XML normalizes literal attribute whitespace before entity
                    // expansion; escaped whitespace must remain distinguishable.
                    let raw = reader
                        .decoder()
                        .decode(attribute.value.as_ref())
                        .map_err(error)?;
                    let raw = raw.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ");
                    let value = quick_xml::escape::unescape(&raw)
                        .map_err(error)?
                        .into_owned();
                    attributes.push((
                        String::from_utf8_lossy(attribute.key.local_name().as_ref()).into_owned(),
                        value,
                    ));
                }
                nodes.push(Node {
                    name: String::from_utf8_lossy(element.local_name().as_ref()).into_owned(),
                    attributes,
                    parent,
                    previous,
                    last_child: None,
                    text: String::new(),
                    text_finished: false,
                });
                if matches!(event, Event::Start(_)) {
                    stack.push(index);
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Text(text) => {
                if let Some(&index) = stack.last()
                    && !nodes[index].text_finished
                {
                    nodes[index]
                        .text
                        .push_str(&text.xml_content().map_err(error)?);
                }
            }
            Event::CData(text) => {
                if let Some(&index) = stack.last()
                    && !nodes[index].text_finished
                {
                    nodes[index]
                        .text
                        .push_str(&text.xml_content().map_err(error)?);
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(&index) = stack.last()
                    && !nodes[index].text_finished
                {
                    let encoded = format!("&{};", String::from_utf8_lossy(reference.as_ref()));
                    nodes[index]
                        .text
                        .push_str(&quick_xml::escape::unescape(&encoded).map_err(error)?);
                }
            }
            Event::Eof => break,
            _ => {
                if let Some(&index) = stack.last() {
                    nodes[index].text_finished = true;
                }
            }
        }
    }
    let mut sheet = StyleSheet::new();
    for node in &nodes {
        if node.name == "style"
            && node
                .attribute("type")
                .is_none_or(|value| value == "text/css")
        {
            sheet.parse_more(&node.text);
        }
    }

    // Keep declaration order rather than implementing a second cascade or
    // shorthand expander. usvg still interprets values, inheritance, shorthands,
    // last normal declarations, and first important declarations itself.
    let mut reader = Reader::from_reader(source.as_slice());
    let mut writer = Writer::new(Vec::with_capacity(source.len()));
    let mut index = 0;
    let mut stack = Vec::new();
    loop {
        let event = reader.read_event().map_err(error)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let node = NodeRef {
                    nodes: &nodes,
                    index,
                };
                let mut style = String::new();
                for rule in &sheet.rules {
                    if rule.selector.matches(&node) {
                        for declaration in &rule.declarations {
                            append(&mut style, *declaration);
                        }
                    }
                }
                if let Some(inline) = nodes[index].attribute("style") {
                    for declaration in DeclarationTokenizer::from(inline) {
                        append(&mut style, declaration);
                    }
                }
                let mut rewritten = element.to_owned();
                rewritten.clear_attributes();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(error)?;
                    if attribute.key.as_ref() != b"style" {
                        rewritten.push_attribute(attribute);
                    }
                }
                if !style.is_empty() {
                    rewritten.push_attribute(("style", style.as_str()));
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(index);
                    writer.write_event(Event::Start(rewritten)).map_err(error)?;
                } else {
                    writer.write_event(Event::Empty(rewritten)).map_err(error)?;
                }
                index += 1;
            }
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_)
                if stack
                    .last()
                    .is_some_and(|&index| nodes[index].name == "style") => {}
            Event::End(end) => {
                stack.pop();
                writer.write_event(Event::End(end)).map_err(error)?;
            }
            Event::Eof => break,
            other => writer.write_event(other).map_err(error)?,
        }
    }
    Ok(writer.into_inner())
}

fn append(style: &mut String, declaration: Declaration<'_>) {
    style.push_str(declaration.name);
    style.push(':');
    style.push_str(declaration.value);
    if declaration.important {
        style.push_str("!important");
    }
    style.push(';');
}
