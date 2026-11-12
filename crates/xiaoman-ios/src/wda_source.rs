//! Flattens the WebDriverAgent page source XML into [`UiNode`]s.
//!
//! Ported from the Kotlin `WdaSource`:
//!
//! - elements are visited in document order and the node `id` is the index in that order;
//! - **bounds are iOS screen points**, clamped to the screen and rounded outwards, exactly like
//!   the Kotlin implementation. This differs from the normalized 0..=1000 bounds the Android
//!   device produces, and from the doc comment of [`xiaoman_api::Bounds`]; the WDA client code
//!   depends on it, so it is kept deliberately. [`xiaoman_api::Point`]s in actions stay normalized;
//! - an element is skipped unless it is `visible="true"` and has a positive, on-screen box;
//! - a node is only kept when it carries text, a name, a value, or is interactive/scrollable.

use std::error::Error;
use std::fmt;

use xiaoman_api::{Bounds, UiNode};

const EDITABLE_TYPES: [&str; 4] = ["TextField", "SecureTextField", "SearchField", "TextView"];
const SCROLLABLE_TYPES: [&str; 4] = ["ScrollView", "Table", "CollectionView", "WebView"];
const CLICKABLE_TYPES: [&str; 6] = ["Button", "Cell", "Link", "Switch", "Key", "Icon"];

/// The page source could not be parsed.
///
/// The message never contains page content: only the parser's own complaint and position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WdaSourceError(pub String);

impl fmt::Display for WdaSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for WdaSourceError {}

/// One kept element with the raw values the device needs for typing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub ui: UiNode,
    /// Element type without the `XCUIElementType` prefix, e.g. `TextField`.
    pub node_type: String,
    pub value: Option<String>,
    pub placeholder: Option<String>,
}

/// Everything the device reads from one page source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    /// Bounds of every `NavigationBar`, used to find the back button.
    pub navigation_bars: Vec<Bounds>,
}

impl Snapshot {
    /// The kept nodes, without the raw values.
    pub fn nodes(&self) -> Vec<UiNode> {
        self.entries.iter().map(|entry| entry.ui.clone()).collect()
    }
}

/// Parses WDA page source XML.
pub struct WdaSource;

impl WdaSource {
    /// Convenience: the nodes only.
    pub fn parse(
        xml: &str,
        screen_width: i32,
        screen_height: i32,
    ) -> Result<Vec<UiNode>, WdaSourceError> {
        Ok(Self::parse_details(xml, screen_width, screen_height)?.nodes())
    }

    /// Nodes plus the navigation bars and the raw typing values.
    ///
    /// A screen of unknown size (`screen_width` or `screen_height` not positive) parses to an
    /// empty snapshot without touching the XML, like the Kotlin implementation.
    pub fn parse_details(
        xml: &str,
        screen_width: i32,
        screen_height: i32,
    ) -> Result<Snapshot, WdaSourceError> {
        if screen_width <= 0 || screen_height <= 0 {
            return Ok(Snapshot::default());
        }
        let document = roxmltree::Document::parse(xml)
            .map_err(|error| WdaSourceError(format!("invalid page source: {error}")))?;
        let mut entries: Vec<Entry> = Vec::new();
        let mut navigation_bars: Vec<Bounds> = Vec::new();
        for element in document.descendants().filter(|node| node.is_element()) {
            let node_type = match attr(&element, "type") {
                Some(value) => value,
                None => element.tag_name().name().to_string(),
            };
            let node_type = strip_prefix(&node_type, "XCUIElementType");
            if attr(&element, "visible").as_deref() != Some("true") {
                continue;
            }
            let Some(bounds) = bounds(&element, screen_width, screen_height) else {
                continue;
            };
            if node_type == "NavigationBar" {
                navigation_bars.push(bounds);
            }
            if node_type == "Application" || node_type == "Window" {
                continue;
            }
            let label = attr(&element, "label");
            let value = attr(&element, "value");
            let placeholder = attr(&element, "placeholderValue");
            let name = attr(&element, "name");
            let text = match &label {
                Some(label) => Some(label.clone()),
                None => value
                    .clone()
                    .filter(|value| Some(value) != placeholder.as_ref()),
            };
            let description = match &name {
                Some(name) if Some(name) != text.as_ref() => Some(name.clone()),
                _ => placeholder
                    .clone()
                    .filter(|placeholder| Some(placeholder) != text.as_ref()),
            };
            let editable = EDITABLE_TYPES.contains(&node_type.as_str());
            let scrollable = SCROLLABLE_TYPES.contains(&node_type.as_str());
            let interactive = CLICKABLE_TYPES.contains(&node_type.as_str())
                || editable
                || (node_type == "Image" && label.is_some());
            if label.is_none() && value.is_none() && name.is_none() && !interactive && !scrollable {
                continue;
            }
            if node_type == "Other" && label.is_none() && value.is_none() && name.is_none() {
                continue;
            }
            entries.push(Entry {
                ui: UiNode {
                    id: entries.len() as i32,
                    text,
                    description,
                    resource_id: None,
                    class_name: Some(node_type.clone()),
                    bounds,
                    clickable: interactive && attr(&element, "enabled").as_deref() == Some("true"),
                    editable,
                    scrollable,
                    focused: attr(&element, "focused").as_deref() == Some("true"),
                    password: node_type == "SecureTextField",
                },
                node_type,
                value,
                placeholder,
            });
        }
        Ok(Snapshot {
            entries,
            navigation_bars,
        })
    }
}

/// Attribute value, or `None` when the attribute is missing or blank.
fn attr(element: &roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    element
        .attribute(name)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn strip_prefix(value: &str, prefix: &str) -> String {
    value.strip_prefix(prefix).unwrap_or(value).to_string()
}

fn bounds(element: &roxmltree::Node<'_, '_>, width: i32, height: i32) -> Option<Bounds> {
    let x = number(element, "x")?;
    let y = number(element, "y")?;
    let w = number(element, "width")?;
    let h = number(element, "height")?;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let screen_width = f64::from(width);
    let screen_height = f64::from(height);
    let left = x.clamp(0.0, screen_width);
    let top = y.clamp(0.0, screen_height);
    let right = (x + w).clamp(0.0, screen_width);
    let bottom = (y + h).clamp(0.0, screen_height);
    if right <= left || bottom <= top {
        return None;
    }
    Some(Bounds {
        left: left.floor() as i32,
        top: top.floor() as i32,
        right: right.ceil() as i32,
        bottom: bottom.ceil() as i32,
    })
}

fn number(element: &roxmltree::Node<'_, '_>, name: &str) -> Option<f64> {
    let value = element.attribute(name)?.trim();
    value.parse::<f64>().ok().filter(|value| value.is_finite())
}
