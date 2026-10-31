//! Namespace-aware, bounded XMP editing. Unknown XML events are retained.

use super::*;
use quick_xml::events::{BytesEnd, BytesStart, BytesText};
use std::collections::HashMap;

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XMP: &str = "http://ns.adobe.com/xap/1.0/";
const DC: &str = "http://purl.org/dc/elements/1.1/";
const LR: &str = "http://ns.adobe.com/lightroom/1.0/";
const RIGHTS: &str = "http://ns.adobe.com/xap/1.0/rights/";
const IPTC: &str = "http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/";

enum Node {
    Element(Element),
    Other(Event<'static>),
}

struct Element {
    start: BytesStart<'static>,
    namespaces: HashMap<String, String>,
    children: Vec<Node>,
    empty: bool,
}

fn invalid(reason: impl Into<String>) -> SpError {
    SpError::CorruptData {
        path: "<XMP>".into(),
        reason: reason.into(),
    }
}

fn text(bytes: &[u8]) -> Result<&str, SpError> {
    std::str::from_utf8(bytes).map_err(|e| invalid(e.to_string()))
}

impl Element {
    fn expanded<'a>(&'a self, name: &'a str, attribute: bool) -> (&'a str, &'a str) {
        match name.split_once(':') {
            Some((prefix, local)) => (
                self.namespaces
                    .get(prefix)
                    .map(String::as_str)
                    .unwrap_or(""),
                local,
            ),
            None => (
                if attribute {
                    ""
                } else {
                    self.namespaces.get("").map(String::as_str).unwrap_or("")
                },
                name,
            ),
        }
    }

    fn is(&self, namespace: &str, local: &str) -> bool {
        text(self.start.name().as_ref())
            .is_ok_and(|name| self.expanded(name, false) == (namespace, local))
    }

    fn attribute(&self, namespace: &str, local: &str) -> Option<String> {
        self.start
            .attributes()
            .filter_map(Result::ok)
            .find_map(|attr| {
                let name = text(attr.key.as_ref()).ok()?;
                (self.expanded(name, true) == (namespace, local))
                    .then(|| attr.unescape_value().ok().map(|v| v.into_owned()))
                    .flatten()
            })
    }

    fn retain_attributes(
        &mut self,
        mut keep: impl FnMut(&str, &str) -> bool,
    ) -> Result<(), SpError> {
        let mut start = BytesStart::new(text(self.start.name().as_ref())?.to_owned());
        for attr in self.start.attributes() {
            let attr = attr.map_err(|e| invalid(e.to_string()))?;
            let name = text(attr.key.as_ref())?;
            let (namespace, local) = self.expanded(name, true);
            if keep(namespace, local) {
                start.push_attribute((attr.key.as_ref(), attr.value.as_ref()));
            }
        }
        self.start = start;
        Ok(())
    }

    fn value(&self) -> String {
        let mut value = String::new();
        for child in &self.children {
            match child {
                Node::Element(element) => value.push_str(&element.value()),
                Node::Other(Event::Text(content)) => {
                    if let Ok(decoded) = content.decode() {
                        if let Ok(unescaped) = quick_xml::escape::unescape(&decoded) {
                            value.push_str(&unescaped);
                        }
                    }
                }
                Node::Other(Event::CData(content)) => {
                    if let Ok(decoded) = content.decode() {
                        value.push_str(&decoded);
                    }
                }
                Node::Other(Event::GeneralRef(reference)) => {
                    if let Ok(decoded) = reference.decode() {
                        if let Ok(unescaped) = quick_xml::escape::unescape(&format!("&{decoded};"))
                        {
                            value.push_str(&unescaped);
                        }
                    }
                }
                _ => {}
            }
        }
        value
    }
}

fn parse(xml: &str) -> Result<Vec<Node>, SpError> {
    if xml.len() > 16 * 1024 * 1024 {
        return Err(invalid("XMP exceeds 16 MiB"));
    }
    if xml.chars().any(|c| {
        !matches!(c, '\t' | '\n' | '\r') && (c < '\u{20}' || matches!(c, '\u{fffe}' | '\u{ffff}'))
    }) {
        return Err(invalid("Invalid XML character"));
    }
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().check_comments = true;
    let mut stack: Vec<Element> = Vec::new();
    let mut nodes = Vec::new();
    let mut root_count = 0;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| invalid(e.to_string()))?
            .into_owned();
        match event {
            Event::Start(start) | Event::Empty(start) => {
                // Empty is identified from the exact bytes because the or-pattern
                // deliberately shares the validation and namespace handling.
                let end = reader.buffer_position() as usize;
                let empty = xml
                    .as_bytes()
                    .get(..end)
                    .is_some_and(|v| v.ends_with(b"/>"));
                if stack.len() >= 128 {
                    return Err(invalid("XMP nesting exceeds 128 levels"));
                }
                if stack.is_empty() {
                    root_count += 1;
                }
                let mut namespaces =
                    stack
                        .last()
                        .map(|e| e.namespaces.clone())
                        .unwrap_or_else(|| {
                            HashMap::from([(
                                "xml".into(),
                                "http://www.w3.org/XML/1998/namespace".into(),
                            )])
                        });
                for attr in start.attributes() {
                    let attr = attr.map_err(|e| invalid(e.to_string()))?;
                    let name = text(attr.key.as_ref())?;
                    let value = attr.unescape_value().map_err(|e| invalid(e.to_string()))?;
                    if name == "xmlns" {
                        namespaces.insert(String::new(), value.into_owned());
                    } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                        namespaces.insert(prefix.into(), value.into_owned());
                    }
                }
                let element = Element {
                    start,
                    namespaces,
                    children: Vec::new(),
                    empty,
                };
                let validate_name = |name: &str| -> Result<(), SpError> {
                    if let Some((prefix, local)) = name.split_once(':') {
                        if local.contains(':')
                            || prefix.is_empty()
                            || local.is_empty()
                            || (prefix != "xmlns" && !element.namespaces.contains_key(prefix))
                        {
                            return Err(invalid(format!(
                                "Undeclared or invalid XML prefix in {name}"
                            )));
                        }
                    }
                    Ok(())
                };
                validate_name(text(element.start.name().as_ref())?)?;
                for attr in element.start.attributes() {
                    validate_name(text(
                        attr.map_err(|e| invalid(e.to_string()))?.key.as_ref(),
                    )?)?;
                }
                if empty {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(Node::Element(element));
                    } else {
                        nodes.push(Node::Element(element));
                    }
                } else {
                    stack.push(element);
                }
            }
            Event::End(_) => {
                let element = stack
                    .pop()
                    .ok_or_else(|| invalid("Unexpected closing element"))?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Element(element));
                } else {
                    nodes.push(Node::Element(element));
                }
            }
            Event::Eof => break,
            Event::DocType(_) => return Err(invalid("DTD declarations are not supported in XMP")),
            event => {
                if let Event::Text(content) = &event {
                    let decoded = content.decode().map_err(|e| invalid(e.to_string()))?;
                    quick_xml::escape::unescape(&decoded).map_err(|e| invalid(e.to_string()))?;
                    if stack.is_empty() && !decoded.trim().is_empty() {
                        return Err(invalid("Text outside XML document"));
                    }
                }
                if let Event::GeneralRef(content) = &event {
                    let decoded = content.decode().map_err(|e| invalid(e.to_string()))?;
                    quick_xml::escape::unescape(&format!("&{decoded};"))
                        .map_err(|e| invalid(e.to_string()))?;
                }
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Other(event));
                } else {
                    nodes.push(Node::Other(event));
                }
            }
        }
    }
    if root_count != 1 || !stack.is_empty() {
        return Err(invalid("XMP must contain one complete XML document"));
    }
    Ok(nodes)
}

fn owned(namespace: &str, local: &str, full_metadata: bool) -> bool {
    (namespace == XMP && local == "Rating")
        || (namespace == SHOPPHOTO_NS && local == "Flag")
        || (full_metadata
            && ((namespace == XMP && local == "Label")
                || (namespace == DC
                    && matches!(local, "description" | "subject" | "creator" | "rights"))
                || (namespace == RIGHTS && local == "UsageTerms")
                || (namespace == LR && local == "hierarchicalSubject")
                || (namespace == "http://ns.adobe.com/exif/1.0/"
                    && matches!(local, "GPSLatitude" | "GPSLongitude"))))
}

fn write_node(writer: &mut quick_xml::Writer<Vec<u8>>, node: &Node) -> Result<(), SpError> {
    let write = |writer: &mut quick_xml::Writer<Vec<u8>>, event| {
        writer
            .write_event(event)
            .map_err(|e| invalid(e.to_string()))
    };
    match node {
        Node::Other(event) => write(writer, event.borrow()),
        Node::Element(element) => {
            if element.empty && element.children.is_empty() {
                return write(writer, Event::Empty(element.start.borrow()));
            }
            write(writer, Event::Start(element.start.borrow()))?;
            for child in &element.children {
                write_node(writer, child)?;
            }
            write(
                writer,
                Event::End(BytesEnd::new(text(element.start.name().as_ref())?)),
            )
        }
    }
}

fn email_element(email: &str) -> Node {
    let mut start = BytesStart::new("spiptc:CiEmailWork");
    start.push_attribute(("xmlns:spiptc", IPTC));
    Node::Element(Element {
        start,
        namespaces: HashMap::from([("spiptc".into(), IPTC.into())]),
        children: vec![Node::Other(Event::Text(BytesText::new(email).into_owned()))],
        empty: false,
    })
}

fn update_contact(element: &mut Element, email: &str) -> Result<(), SpError> {
    element.retain_attributes(|namespace, local| !(namespace == IPTC && local == "CiEmailWork"))?;
    element
        .children
        .retain(|child| !matches!(child, Node::Element(e) if e.is(IPTC, "CiEmailWork")));
    if let Some(description) = element.children.iter_mut().find_map(|child| match child {
        Node::Element(e) if e.is(RDF, "Description") => Some(e),
        _ => None,
    }) {
        update_contact(description, email)?;
    } else if !email.is_empty() {
        element.children.push(email_element(email));
    }
    Ok(())
}

fn rdf_mut(nodes: &mut [Node]) -> Option<&mut Element> {
    for node in nodes {
        if let Node::Element(element) = node {
            if element.is(RDF, "RDF") {
                return Some(element);
            }
            if let Some(found) = rdf_mut(&mut element.children) {
                return Some(found);
            }
        }
    }
    None
}

/// Merge only ShopPhoto-owned properties, preserving all other namespaces.
pub(super) fn merge(
    existing: &str,
    annotations: Annotations,
    metadata: &DescriptiveMetadata,
    full_metadata: bool,
) -> Result<String, SpError> {
    let mut document = parse(existing)?;
    let rdf = rdf_mut(&mut document).ok_or_else(|| invalid("XMP has no rdf:RDF document"))?;
    let mut has_contact = false;
    for child in &mut rdf.children {
        let Node::Element(description) = child else {
            continue;
        };
        if !description.is(RDF, "Description")
            || description
                .attribute(RDF, "about")
                .is_some_and(|value| !value.is_empty())
        {
            continue;
        }
        description
            .retain_attributes(|namespace, local| !owned(namespace, local, full_metadata))?;
        description.children.retain(|child| !matches!(child, Node::Element(e) if text(e.start.name().as_ref()).is_ok_and(|name| { let (namespace, local) = e.expanded(name, false); owned(namespace, local, full_metadata) })));
        if full_metadata {
            for child in &mut description.children {
                if let Node::Element(contact) = child {
                    if contact.is(IPTC, "CreatorContactInfo") {
                        update_contact(contact, &metadata.contact)?;
                        has_contact = true;
                    }
                }
            }
        }
    }
    let mut metadata = metadata.clone();
    if has_contact {
        metadata.contact.clear();
    }
    let mut generated = parse(&to_packet_with_metadata(annotations, &metadata))?;
    let generated_rdf =
        rdf_mut(&mut generated).ok_or_else(|| invalid("Generated XMP has no RDF document"))?;
    if !full_metadata {
        for child in &mut generated_rdf.children {
            if let Node::Element(description) = child {
                description.retain_attributes(|namespace, local| {
                    !owned(namespace, local, true) || owned(namespace, local, false)
                })?;
                description.children.clear();
            }
        }
    }
    // Declare RDF on inserted descriptions: the foreign document may call its
    // RDF prefix something other than `rdf`, or bind `rdf` to another URI.
    for child in &mut generated_rdf.children {
        if let Node::Element(element) = child {
            element.start.push_attribute(("xmlns:rdf", RDF));
        }
    }
    rdf.children.append(&mut generated_rdf.children);
    let mut writer = quick_xml::Writer::new(Vec::new());
    for node in &document {
        write_node(&mut writer, node)?;
    }
    String::from_utf8(writer.into_inner()).map_err(|e| invalid(e.to_string()))
}

fn array_values(element: &Element) -> Vec<String> {
    let mut values = Vec::new();
    for child in &element.children {
        if let Node::Element(child) = child {
            if child.is(RDF, "li") {
                values.push(child.value());
            } else {
                values.extend(array_values(child));
            }
        }
    }
    values
}

pub(super) fn read(xml: &str) -> Result<(Option<Annotations>, DescriptiveMetadata), SpError> {
    let mut document = parse(xml)?;
    let rdf = rdf_mut(&mut document).ok_or_else(|| invalid("XMP has no RDF document"))?;
    let mut rating = None;
    let mut flag = None;
    let mut metadata = DescriptiveMetadata::default();
    let mut lat = None;
    let mut lon = None;
    let mut apply = |namespace: &str, name: &str, value: &str| match (namespace, name) {
        (XMP, "Rating") => rating = value.trim().parse::<i32>().ok(),
        (SHOPPHOTO_NS, "Flag") => flag = parse_flag(value),
        (XMP, "Label") => {
            metadata.color_label = match value {
                "Red" => crate::ColorLabel::Red,
                "Yellow" => crate::ColorLabel::Yellow,
                "Green" => crate::ColorLabel::Green,
                "Blue" => crate::ColorLabel::Blue,
                "Purple" => crate::ColorLabel::Purple,
                _ => crate::ColorLabel::None,
            }
        }
        (DC, "description") => metadata.caption = value.to_owned(),
        (DC, "creator") => metadata.creator = value.to_owned(),
        (DC, "rights") => metadata.copyright = value.to_owned(),
        (RIGHTS, "UsageTerms") => metadata.rights = value.to_owned(),
        (IPTC, "CiEmailWork") => metadata.contact = value.to_owned(),
        ("http://ns.adobe.com/exif/1.0/", "GPSLatitude") => lat = coordinate(value, true),
        ("http://ns.adobe.com/exif/1.0/", "GPSLongitude") => lon = coordinate(value, false),
        _ => {}
    };
    fn visit(
        element: &Element,
        apply: &mut impl FnMut(&str, &str, &str),
        keywords: &mut Vec<String>,
    ) {
        for attr in element.start.attributes().filter_map(Result::ok) {
            if let (Ok(name), Ok(value)) = (text(attr.key.as_ref()), attr.unescape_value()) {
                let (namespace, local) = element.expanded(name, true);
                apply(namespace, local, &value);
            }
        }
        for child in &element.children {
            if let Node::Element(child) = child {
                if let Ok(name) = text(child.start.name().as_ref()) {
                    let (namespace, local) = child.expanded(name, false);
                    let values = array_values(child);
                    if (namespace == DC && local == "subject")
                        || (namespace == LR && local == "hierarchicalSubject")
                    {
                        for value in values {
                            if !keywords.contains(&value) {
                                keywords.push(value);
                            }
                        }
                    } else {
                        apply(
                            namespace,
                            local,
                            &values.first().cloned().unwrap_or_else(|| child.value()),
                        );
                    }
                }
                if child.is(IPTC, "CreatorContactInfo") || child.is(RDF, "Description") {
                    visit(child, apply, keywords);
                }
            }
        }
    }
    let mut keywords = Vec::new();
    for child in &rdf.children {
        if let Node::Element(description) = child {
            if description.is(RDF, "Description")
                && description
                    .attribute(RDF, "about")
                    .is_none_or(|value| value.is_empty())
            {
                visit(description, &mut apply, &mut keywords);
            }
        }
    }
    metadata.keywords = keywords;
    metadata.gps = lat.zip(lon).map(|(lat, lon)| [lat, lon]);
    let annotations = (rating.is_some() || flag.is_some()).then(|| Annotations {
        rating: rating.unwrap_or(0).clamp(0, 5) as u8,
        flag: flag.unwrap_or(if rating.unwrap_or(0) < 0 {
            Flag::Reject
        } else {
            Flag::None
        }),
    });
    Ok((annotations, metadata))
}

fn coordinate(value: &str, latitude: bool) -> Option<f64> {
    let value = value.trim();
    let direction = value.chars().last()?;
    let numeric = value.trim_end_matches(['N', 'S', 'E', 'W']);
    let parts = numeric
        .split(',')
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut number = match parts.as_slice() {
        [degrees] => *degrees,
        [degrees, minutes] => degrees + minutes / 60.0,
        [degrees, minutes, seconds] => degrees + minutes / 60.0 + seconds / 3600.0,
        _ => return None,
    };
    if matches!(direction, 'S' | 'W') {
        number = -number.abs();
    }
    let limit = if latitude { 90.0 } else { 180.0 };
    (number.is_finite() && number.abs() <= limit).then_some(number)
}
