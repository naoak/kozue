//! IR layers for kozue.
//!
//! - Semantic IR: [`Diagram`] and its variants describe *what* the diagram is.
//! - Scene IR: [`Scene`] and [`SceneItem`] describe *drawing primitives* only,
//!   already laid out. The renderer only ever sees the Scene IR.

use std::{borrow::Borrow, collections::BTreeMap, fmt};

use indexmap::IndexMap;
use serde::{de, ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// Semantic IR
// ---------------------------------------------------------------------------

/// Version of the serialized [`IrDocument`] schema.
///
/// The wire representation is an integer so schema negotiation does not rely
/// on Rust enum variant names. Unknown versions are rejected during
/// deserialization rather than being interpreted as the current schema.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IrSchemaVersion {
    V1,
    V2,
    V3,
    #[default]
    V4,
}

/// Schema version produced by newly constructed IR documents.
pub const CURRENT_IR_SCHEMA_VERSION: IrSchemaVersion = IrSchemaVersion::V4;

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

impl Serialize for IrSchemaVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            IrSchemaVersion::V1 => serializer.serialize_u8(1),
            IrSchemaVersion::V2 => serializer.serialize_u8(2),
            IrSchemaVersion::V3 => serializer.serialize_u8(3),
            IrSchemaVersion::V4 => serializer.serialize_u8(4),
        }
    }
}

impl<'de> Deserialize<'de> for IrSchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = u64::deserialize(deserializer)?;
        match version {
            1 => Ok(IrSchemaVersion::V1),
            2 => Ok(IrSchemaVersion::V2),
            3 => Ok(IrSchemaVersion::V3),
            4 => Ok(IrSchemaVersion::V4),
            other => Err(de::Error::custom(format!(
                "unsupported IR schema version {other}"
            ))),
        }
    }
}

/// Stable identifier for a semantic diagram element.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ElementId(String);

impl ElementId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for ElementId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ElementId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<&String> for ElementId {
    fn from(value: &String) -> Self {
        Self(value.clone())
    }
}

impl AsRef<str> for ElementId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ElementId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ElementId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Accessibility metadata independent of any renderer.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessibilityMetadata {
    #[serde(deserialize_with = "deserialize_required_option")]
    pub title: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub description: Option<String>,
}

/// Metadata attached to a complete IR document.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagramMetadata {
    #[serde(deserialize_with = "deserialize_required_option")]
    pub name: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub title: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub description: Option<String>,
    pub accessibility: AccessibilityMetadata,
}

/// Deterministically ordered, namespaced extension data.
///
/// Current schemas intentionally expose no mutation API: frontends always
/// create an empty value until extension ownership and validation rules are
/// defined.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Extensions(BTreeMap<String, BTreeMap<String, serde_json::Value>>);

impl Extensions {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A versioned semantic IR document.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct IrDocument {
    schema_version: IrSchemaVersion,
    pub metadata: DiagramMetadata,
    pub diagram: Diagram,
    pub annotations: Vec<Annotation>,
    pub extensions: Extensions,
}

impl Serialize for IrDocument {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut document = serializer.serialize_struct("IrDocument", 5)?;
        document.serialize_field("schema_version", &CURRENT_IR_SCHEMA_VERSION)?;
        document.serialize_field("metadata", &self.metadata)?;
        document.serialize_field("diagram", &self.diagram)?;
        document.serialize_field("annotations", &self.annotations)?;
        document.serialize_field("extensions", &self.extensions)?;
        document.end()
    }
}

impl IrDocument {
    pub fn new(diagram: Diagram) -> Self {
        Self {
            schema_version: CURRENT_IR_SCHEMA_VERSION,
            metadata: DiagramMetadata::default(),
            diagram,
            annotations: Vec::new(),
            extensions: Extensions::default(),
        }
    }

    pub fn into_diagram(self) -> Diagram {
        self.diagram
    }

    pub fn schema_version(&self) -> IrSchemaVersion {
        self.schema_version
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrDocumentV1 {
    schema_version: IrSchemaVersion,
    metadata: DiagramMetadata,
    diagram: Diagram,
    extensions: Extensions,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IrDocumentWithAnnotations {
    schema_version: IrSchemaVersion,
    metadata: DiagramMetadata,
    diagram: Diagram,
    annotations: Vec<Annotation>,
    extensions: Extensions,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IrDocumentWire {
    Annotated(IrDocumentWithAnnotations),
    V1(IrDocumentV1),
}

fn direction_supported_in(version: IrSchemaVersion, direction: Direction) -> bool {
    match direction {
        Direction::Down | Direction::Right => true,
        Direction::Up | Direction::Left => {
            matches!(version, IrSchemaVersion::V3 | IrSchemaVersion::V4)
        }
    }
}

fn node_kind_supported_in(version: IrSchemaVersion, kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Default => true,
        NodeKind::Rectangle | NodeKind::RoundedRectangle => version == IrSchemaVersion::V4,
    }
}

fn diagram_supported_in(version: IrSchemaVersion, diagram: &Diagram) -> bool {
    match diagram {
        Diagram::Graph(graph) => {
            direction_supported_in(version, graph.direction)
                && graph
                    .nodes
                    .values()
                    .all(|node| node_kind_supported_in(version, &node.kind))
        }
        Diagram::Class(class) => direction_supported_in(version, class.direction),
        _ => true,
    }
}

impl<'de> Deserialize<'de> for IrDocument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match IrDocumentWire::deserialize(deserializer)? {
            IrDocumentWire::V1(wire) if wire.schema_version == IrSchemaVersion::V1 => {
                if !diagram_supported_in(IrSchemaVersion::V1, &wire.diagram) {
                    return Err(de::Error::custom(
                        "IR schema version 1 does not support this diagram direction or node kind",
                    ));
                }
                Ok(Self {
                    schema_version: CURRENT_IR_SCHEMA_VERSION,
                    metadata: wire.metadata,
                    diagram: wire.diagram,
                    annotations: Vec::new(),
                    extensions: wire.extensions,
                })
            }
            IrDocumentWire::Annotated(wire) if wire.schema_version == IrSchemaVersion::V2 => {
                if !diagram_supported_in(IrSchemaVersion::V2, &wire.diagram) {
                    return Err(de::Error::custom(
                        "IR schema version 2 does not support this diagram direction or node kind",
                    ));
                }
                Ok(Self {
                    schema_version: CURRENT_IR_SCHEMA_VERSION,
                    metadata: wire.metadata,
                    diagram: wire.diagram,
                    annotations: wire.annotations,
                    extensions: wire.extensions,
                })
            }
            IrDocumentWire::Annotated(wire) if wire.schema_version == IrSchemaVersion::V3 => {
                if !diagram_supported_in(IrSchemaVersion::V3, &wire.diagram) {
                    return Err(de::Error::custom(
                        "IR schema version 3 does not support this diagram direction or node kind",
                    ));
                }
                Ok(Self {
                    schema_version: CURRENT_IR_SCHEMA_VERSION,
                    metadata: wire.metadata,
                    diagram: wire.diagram,
                    annotations: wire.annotations,
                    extensions: wire.extensions,
                })
            }
            IrDocumentWire::Annotated(wire) if wire.schema_version == IrSchemaVersion::V4 => {
                if !diagram_supported_in(IrSchemaVersion::V4, &wire.diagram) {
                    return Err(de::Error::custom(
                        "IR schema version 4 does not support this diagram direction or node kind",
                    ));
                }
                Ok(Self {
                    schema_version: CURRENT_IR_SCHEMA_VERSION,
                    metadata: wire.metadata,
                    diagram: wire.diagram,
                    annotations: wire.annotations,
                    extensions: wire.extensions,
                })
            }
            IrDocumentWire::V1(_) => Err(de::Error::custom(
                "IR schema versions 2, 3, and 4 require an `annotations` field",
            )),
            IrDocumentWire::Annotated(_) => Err(de::Error::custom(
                "IR schema version 1 must not contain an `annotations` field",
            )),
        }
    }
}

/// Renderer-independent annotation attached to a diagram or its elements.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    pub id: ElementId,
    pub target: AnnotationTarget,
    pub kind: AnnotationKind,
}

impl Annotation {
    pub fn new(id: impl Into<ElementId>, target: AnnotationTarget, kind: AnnotationKind) -> Self {
        Self {
            id: id.into(),
            target,
            kind,
        }
    }
}

/// Target of an [`Annotation`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum AnnotationTarget {
    Diagram,
    Element(ElementId),
    Elements(Vec<ElementId>),
}

/// Placement preference for note annotations.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NotePlacement {
    #[default]
    Auto,
    Left,
    Right,
    Above,
    Below,
    Over,
}

/// Payload of an [`Annotation`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum AnnotationKind {
    Note {
        text: String,
        placement: NotePlacement,
    },
    Link {
        url: String,
    },
    Tooltip {
        text: String,
    },
    Stereotype {
        name: String,
    },
    Tag {
        name: String,
        #[serde(deserialize_with = "deserialize_required_option")]
        value: Option<String>,
    },
}

/// Top-level semantic diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Diagram {
    Graph(GraphDiagram),
    Sequence(SequenceDiagram),
    State(StateDiagram),
    Class(ClassDiagram),
    Er(ErDiagram),
}

/// Layout direction for a graph diagram.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Direction {
    #[default]
    Down,
    Right,
    Up,
    Left,
}

/// A graph diagram: a set of nodes and directed edges between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphDiagram {
    pub direction: Direction,
    /// Nodes keyed by their stable string ID. Iteration order is insertion
    /// (declaration) order, which the layout relies on for determinism.
    pub nodes: IndexMap<ElementId, Node>,
    pub edges: Vec<Edge>,
}

impl GraphDiagram {
    pub fn new(direction: Direction) -> Self {
        GraphDiagram {
            direction,
            nodes: IndexMap::new(),
            edges: Vec::new(),
        }
    }
}

/// A sequence diagram: ordered list of participants and message items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SequenceDiagram {
    pub participants: IndexMap<ElementId, Participant>,
    pub items: Vec<SequenceItem>,
}

impl SequenceDiagram {
    pub fn new() -> Self {
        SequenceDiagram {
            participants: IndexMap::new(),
            items: Vec::new(),
        }
    }
}

impl Default for SequenceDiagram {
    fn default() -> Self {
        Self::new()
    }
}

/// A participant in a sequence diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    pub id: ElementId,
    pub label: String,
}

impl Participant {
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Participant {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// An item in a sequence diagram body.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SequenceItem {
    Message(Message),
}

/// The line style of a sequence message arrow.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineStyle {
    Solid,
    Dashed,
}

/// A message (arrow) between two participants in a sequence diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub from: ElementId,
    pub to: ElementId,
    pub label: Option<String>,
    pub line: LineStyle,
    pub arrow: ArrowType,
}

impl Message {
    pub fn new(
        from: impl Into<ElementId>,
        to: impl Into<ElementId>,
        label: Option<String>,
        line: LineStyle,
        arrow: ArrowType,
    ) -> Self {
        Message {
            from: from.into(),
            to: to.into(),
            label,
            line,
            arrow,
        }
    }
}

/// The kind of a node.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Default,
    Rectangle,
    RoundedRectangle,
}

/// A node with a stable ID, a display label, and a kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: ElementId,
    pub label: String,
    pub kind: NodeKind,
}

impl Node {
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Node {
            id: id.into(),
            label: label.into(),
            kind: NodeKind::Default,
        }
    }

    pub fn with_kind(id: impl Into<ElementId>, label: impl Into<String>, kind: NodeKind) -> Self {
        Node {
            id: id.into(),
            label: label.into(),
            kind,
        }
    }
}

/// The arrow style drawn at the target end of an edge.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ArrowType {
    #[default]
    Triangle,
    /// No arrowhead — plain line. Used for Mermaid `---` edges.
    None,
}

/// A directed edge from one node to another, with an optional label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: ElementId,
    pub to: ElementId,
    pub label: Option<String>,
    pub arrow: ArrowType,
}

impl Edge {
    pub fn new(
        from: impl Into<ElementId>,
        to: impl Into<ElementId>,
        label: Option<String>,
        arrow: ArrowType,
    ) -> Self {
        Edge {
            from: from.into(),
            to: to.into(),
            label,
            arrow,
        }
    }
}

/// A state diagram: a set of states and transitions between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateDiagram {
    pub states: IndexMap<ElementId, State>,
    pub transitions: Vec<Transition>,
}

impl StateDiagram {
    pub fn new() -> Self {
        StateDiagram {
            states: IndexMap::new(),
            transitions: Vec::new(),
        }
    }
}

impl Default for StateDiagram {
    fn default() -> Self {
        Self::new()
    }
}

/// A state in a state diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub id: ElementId,
    pub label: String,
}

impl State {
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        State {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// An endpoint of a transition.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Endpoint {
    Initial,
    Final,
    State(ElementId),
}

/// A transition between two endpoints in a state diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub from: Endpoint,
    pub to: Endpoint,
    pub label: Option<String>,
}

impl Transition {
    pub fn new(from: Endpoint, to: Endpoint, label: Option<String>) -> Self {
        Transition { from, to, label }
    }
}

// ---------------------------------------------------------------------------
// Class / ER diagrams
// ---------------------------------------------------------------------------

/// The symbol drawn at one end of a relation line. Shared by [`ClassDiagram`]
/// and [`ErDiagram`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndMarker {
    None,
    /// UML generalization/realization (inheritance).
    HollowTriangle,
    /// UML association/dependency direction (open V shape).
    OpenArrow,
    /// UML composition.
    FilledDiamond,
    /// UML aggregation.
    HollowDiamond,
    /// ER crow's foot: exactly one (`‖`).
    ErOne,
    /// ER crow's foot: many (`<`).
    ErMany,
    /// ER crow's foot: zero or one (`o|`).
    ErZeroOrOne,
    /// ER crow's foot: one or many (`|<`).
    ErOneOrMany,
    /// ER crow's foot: zero or many (`o<`).
    ErZeroOrMany,
}

/// A class diagram: a set of classes/interfaces and relations between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassDiagram {
    pub direction: Direction,
    /// Classes keyed by their stable string ID. Iteration order is insertion
    /// (declaration) order, which the layout relies on for determinism.
    pub classes: IndexMap<ElementId, ClassNode>,
    pub relations: Vec<ClassRelation>,
}

impl ClassDiagram {
    pub fn new(direction: Direction) -> Self {
        ClassDiagram {
            direction,
            classes: IndexMap::new(),
            relations: Vec::new(),
        }
    }
}

/// A class (or interface/abstract class/enum) in a class diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassNode {
    pub id: ElementId,
    pub name: String,
    /// `"interface"` / `"abstract"` / `"enumeration"` (from `<<...>>` annotations).
    pub stereotype: Option<String>,
    /// Pre-formatted display lines, e.g. `"+name: String"`.
    pub attributes: Vec<String>,
    /// Pre-formatted display lines, e.g. `"+getName(): String"`.
    pub methods: Vec<String>,
}

impl ClassNode {
    pub fn new(id: impl Into<ElementId>, name: impl Into<String>) -> Self {
        ClassNode {
            id: id.into(),
            name: name.into(),
            stereotype: None,
            attributes: Vec::new(),
            methods: Vec::new(),
        }
    }
}

/// A relation (edge) between two classes in a class diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassRelation {
    pub from: ElementId,
    pub to: ElementId,
    pub from_marker: EndMarker,
    pub to_marker: EndMarker,
    /// Solid / Dashed (reuses the sequence-diagram line style).
    pub line: LineStyle,
    pub label: Option<String>,
    /// Multiplicity at the `from` end, e.g. `"1"` or `"0..*"`.
    pub from_mult: Option<String>,
    /// Multiplicity at the `to` end.
    pub to_mult: Option<String>,
}

impl ClassRelation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        from: impl Into<ElementId>,
        to: impl Into<ElementId>,
        from_marker: EndMarker,
        to_marker: EndMarker,
        line: LineStyle,
        label: Option<String>,
        from_mult: Option<String>,
        to_mult: Option<String>,
    ) -> Self {
        ClassRelation {
            from: from.into(),
            to: to.into(),
            from_marker,
            to_marker,
            line,
            label,
            from_mult,
            to_mult,
        }
    }
}

/// An entity-relationship (ER) diagram: entities and relations between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErDiagram {
    /// Entities keyed by their stable string ID. Iteration order is insertion
    /// (declaration) order, which the layout relies on for determinism.
    pub entities: IndexMap<ElementId, ErEntity>,
    pub relations: Vec<ErRelation>,
}

impl ErDiagram {
    pub fn new() -> Self {
        ErDiagram {
            entities: IndexMap::new(),
            relations: Vec::new(),
        }
    }
}

impl Default for ErDiagram {
    fn default() -> Self {
        Self::new()
    }
}

/// An entity in an ER diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErEntity {
    pub id: ElementId,
    pub name: String,
    pub attributes: Vec<ErAttribute>,
}

impl ErEntity {
    pub fn new(id: impl Into<ElementId>, name: impl Into<String>) -> Self {
        ErEntity {
            id: id.into(),
            name: name.into(),
            attributes: Vec::new(),
        }
    }
}

/// A single attribute row of an ER entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErAttribute {
    /// `"string"`, `"int"`, etc. Empty string if not specified.
    pub type_name: String,
    pub name: String,
    /// `"PK"` / `"FK"` / `"UK"`.
    pub keys: Vec<String>,
    pub comment: Option<String>,
}

impl ErAttribute {
    pub fn new(
        type_name: impl Into<String>,
        name: impl Into<String>,
        keys: Vec<String>,
        comment: Option<String>,
    ) -> Self {
        ErAttribute {
            type_name: type_name.into(),
            name: name.into(),
            keys,
            comment,
        }
    }
}

/// A relation (edge) between two entities in an ER diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErRelation {
    pub from: ElementId,
    pub to: ElementId,
    pub from_marker: EndMarker,
    pub to_marker: EndMarker,
    pub label: Option<String>,
    /// Solid = identifying relationship, Dashed = non-identifying.
    pub line: LineStyle,
}

impl ErRelation {
    pub fn new(
        from: impl Into<ElementId>,
        to: impl Into<ElementId>,
        from_marker: EndMarker,
        to_marker: EndMarker,
        label: Option<String>,
        line: LineStyle,
    ) -> Self {
        ErRelation {
            from: from.into(),
            to: to.into(),
            from_marker,
            to_marker,
            label,
            line,
        }
    }
}

// ---------------------------------------------------------------------------
// Scene IR
// ---------------------------------------------------------------------------

/// A fully laid-out scene: a flat/nested list of drawing primitives.
/// Coordinates are `f64`, units are pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub width: f64,
    pub height: f64,
    pub items: Vec<SceneItem>,
}

/// Horizontal text alignment.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    Start,
    Middle,
    End,
}

/// A single drawing primitive.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SceneItem {
    Rect(Rect),
    Path(Path),
    Text(Text),
    Group(Group),
}

/// A rounded rectangle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub rx: f64,
}

/// A polyline / open path. Optionally filled (used for arrowheads).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Path {
    pub points: Vec<(f64, f64)>,
    /// When `true`, the path is closed and filled (e.g. an arrowhead).
    pub filled: bool,
    /// When `true`, the stroke is rendered as a dashed line.
    pub dashed: bool,
}

/// A text run positioned at `(x, y)` with the given alignment, and its
/// measured dimensions used for viewBox bounds calculation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Text {
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub align: TextAlign,
    pub content: String,
    /// Measured width of the text run (in px). Used by the renderer to
    /// compute scene bounds so that text is not clipped in the viewBox.
    pub text_width: f64,
    /// Measured height of the text run (in px).
    pub text_height: f64,
}

/// A named group of items (for structure; no transform in M0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    pub items: Vec<SceneItem>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const EMPTY_GRAPH_DOCUMENT_V1: &str = r#"{"schema_version":1,"metadata":{"name":null,"title":null,"description":null,"accessibility":{"title":null,"description":null}},"diagram":{"Graph":{"direction":"Down","nodes":{},"edges":[]}},"extensions":{}}"#;
    const EMPTY_GRAPH_DOCUMENT_V2: &str = r#"{"schema_version":2,"metadata":{"name":null,"title":null,"description":null,"accessibility":{"title":null,"description":null}},"diagram":{"Graph":{"direction":"Down","nodes":{},"edges":[]}},"annotations":[],"extensions":{}}"#;
    const EMPTY_GRAPH_DOCUMENT_V3: &str = r#"{"schema_version":3,"metadata":{"name":null,"title":null,"description":null,"accessibility":{"title":null,"description":null}},"diagram":{"Graph":{"direction":"Down","nodes":{},"edges":[]}},"annotations":[],"extensions":{}}"#;
    const EMPTY_GRAPH_DOCUMENT_V4: &str = r#"{"schema_version":4,"metadata":{"name":null,"title":null,"description":null,"accessibility":{"title":null,"description":null}},"diagram":{"Graph":{"direction":"Down","nodes":{},"edges":[]}},"annotations":[],"extensions":{}}"#;

    #[test]
    fn element_id_is_transparent_and_supports_string_lookup() {
        let id = ElementId::new("alpha");
        assert_eq!(id.as_str(), "alpha");
        assert_eq!(id.to_string(), "alpha");
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""alpha""#);
        assert_eq!(serde_json::from_str::<ElementId>(r#""alpha""#).unwrap(), id);

        let mut graph = GraphDiagram::new(Direction::Down);
        graph
            .nodes
            .insert(id.clone(), Node::new(id.clone(), "Alpha"));
        assert_eq!(graph.nodes.get("alpha").unwrap().id, id);
        assert_eq!(
            ElementId::from(String::from("owned")).into_string(),
            "owned"
        );
        let borrowed = String::from("borrowed");
        assert_eq!(ElementId::from(&borrowed).as_str(), "borrowed");
    }

    #[test]
    fn schema_version_is_numeric_and_rejects_unknown_versions() {
        assert_eq!(serde_json::to_value(IrSchemaVersion::V1).unwrap(), json!(1));
        assert_eq!(
            serde_json::from_value::<IrSchemaVersion>(json!(1)).unwrap(),
            IrSchemaVersion::V1
        );
        assert_eq!(serde_json::to_value(IrSchemaVersion::V2).unwrap(), json!(2));
        assert_eq!(serde_json::to_value(IrSchemaVersion::V3).unwrap(), json!(3));
        assert_eq!(serde_json::to_value(IrSchemaVersion::V4).unwrap(), json!(4));
        let error = serde_json::from_value::<IrSchemaVersion>(json!(5)).unwrap_err();
        assert!(error
            .to_string()
            .contains("unsupported IR schema version 5"));
    }

    #[test]
    fn all_directions_round_trip() {
        for direction in [
            Direction::Down,
            Direction::Right,
            Direction::Up,
            Direction::Left,
        ] {
            let json = serde_json::to_string(&direction).unwrap();
            assert_eq!(serde_json::from_str::<Direction>(&json).unwrap(), direction);
        }
    }

    #[test]
    fn document_round_trip_uses_empty_deterministic_extensions() {
        let document = IrDocument::new(Diagram::Graph(GraphDiagram::new(Direction::Down)));
        assert!(document.extensions.is_empty());
        assert_eq!(document.schema_version(), CURRENT_IR_SCHEMA_VERSION);

        let serialized = serde_json::to_string(&document).unwrap();
        assert_eq!(serialized, EMPTY_GRAPH_DOCUMENT_V4);
        assert_eq!(
            serde_json::from_str::<IrDocument>(&serialized).unwrap(),
            document
        );
    }

    #[test]
    fn v1_v2_and_v3_documents_are_upgraded_to_v4() {
        for fixture in [
            EMPTY_GRAPH_DOCUMENT_V1,
            EMPTY_GRAPH_DOCUMENT_V2,
            EMPTY_GRAPH_DOCUMENT_V3,
        ] {
            let document = serde_json::from_str::<IrDocument>(fixture).unwrap();
            assert_eq!(document.schema_version(), IrSchemaVersion::V4);
            assert!(document.annotations.is_empty());
            assert_eq!(
                serde_json::to_string(&document).unwrap(),
                EMPTY_GRAPH_DOCUMENT_V4
            );
        }

        let mut v2_without_annotations: serde_json::Value =
            serde_json::from_str(EMPTY_GRAPH_DOCUMENT_V2).unwrap();
        v2_without_annotations
            .as_object_mut()
            .unwrap()
            .remove("annotations");
        assert!(serde_json::from_value::<IrDocument>(v2_without_annotations).is_err());

        let mut invalid: serde_json::Value = serde_json::from_str(EMPTY_GRAPH_DOCUMENT_V1).unwrap();
        invalid["annotations"] = json!([]);
        assert!(serde_json::from_value::<IrDocument>(invalid).is_err());
    }

    #[test]
    fn reverse_directions_require_schema_v3_for_graph_and_class() {
        let diagrams = |direction: Direction| {
            [
                Diagram::Graph(GraphDiagram::new(direction)),
                Diagram::Class(ClassDiagram::new(direction)),
            ]
        };

        for direction in [Direction::Up, Direction::Left] {
            for diagram in diagrams(direction) {
                for fixture in [EMPTY_GRAPH_DOCUMENT_V1, EMPTY_GRAPH_DOCUMENT_V2] {
                    let mut value: serde_json::Value = serde_json::from_str(fixture).unwrap();
                    value["diagram"] = serde_json::to_value(&diagram).unwrap();
                    assert!(
                        serde_json::from_value::<IrDocument>(value).is_err(),
                        "legacy schema accepted {diagram:?}"
                    );
                }

                for fixture in [EMPTY_GRAPH_DOCUMENT_V3, EMPTY_GRAPH_DOCUMENT_V4] {
                    let mut value: serde_json::Value = serde_json::from_str(fixture).unwrap();
                    value["diagram"] = serde_json::to_value(&diagram).unwrap();
                    assert_eq!(
                        serde_json::from_value::<IrDocument>(value)
                            .unwrap()
                            .into_diagram(),
                        diagram
                    );
                }
            }
        }
    }

    #[test]
    fn explicit_node_kinds_require_schema_v4() {
        for kind in [NodeKind::Rectangle, NodeKind::RoundedRectangle] {
            let mut graph = GraphDiagram::new(Direction::Down);
            graph
                .nodes
                .insert("a".into(), Node::with_kind("a", "A", kind.clone()));
            let diagram = Diagram::Graph(graph);

            for fixture in [
                EMPTY_GRAPH_DOCUMENT_V1,
                EMPTY_GRAPH_DOCUMENT_V2,
                EMPTY_GRAPH_DOCUMENT_V3,
            ] {
                let mut value: serde_json::Value = serde_json::from_str(fixture).unwrap();
                value["diagram"] = serde_json::to_value(&diagram).unwrap();
                assert!(serde_json::from_value::<IrDocument>(value).is_err());
            }

            let mut value: serde_json::Value =
                serde_json::from_str(EMPTY_GRAPH_DOCUMENT_V4).unwrap();
            value["diagram"] = serde_json::to_value(&diagram).unwrap();
            assert_eq!(
                serde_json::from_value::<IrDocument>(value)
                    .unwrap()
                    .into_diagram(),
                diagram
            );
        }
    }

    #[test]
    fn v4_annotations_round_trip_in_declaration_order() {
        let mut document = IrDocument::new(Diagram::Graph(GraphDiagram::new(Direction::Down)));
        document.annotations = vec![
            Annotation::new(
                "note-2",
                AnnotationTarget::Elements(vec!["b".into(), "a".into()]),
                AnnotationKind::Note {
                    text: "second".to_string(),
                    placement: NotePlacement::Right,
                },
            ),
            Annotation::new(
                "link-1",
                AnnotationTarget::Diagram,
                AnnotationKind::Link {
                    url: "https://example.com".to_string(),
                },
            ),
            Annotation::new(
                "tooltip-1",
                AnnotationTarget::Element("a".into()),
                AnnotationKind::Tooltip {
                    text: "details".to_string(),
                },
            ),
            Annotation::new(
                "stereotype-1",
                AnnotationTarget::Element("a".into()),
                AnnotationKind::Stereotype {
                    name: "service".to_string(),
                },
            ),
            Annotation::new(
                "tag-1",
                AnnotationTarget::Diagram,
                AnnotationKind::Tag {
                    name: "owner".to_string(),
                    value: Some("platform".to_string()),
                },
            ),
        ];

        let serialized = serde_json::to_string(&document).unwrap();
        assert!(serialized.find("note-2").unwrap() < serialized.find("link-1").unwrap());
        let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            value["annotations"],
            json!([
                {
                    "id": "note-2",
                    "target": {"Elements": ["b", "a"]},
                    "kind": {"Note": {"text": "second", "placement": "Right"}}
                },
                {
                    "id": "link-1",
                    "target": "Diagram",
                    "kind": {"Link": {"url": "https://example.com"}}
                },
                {
                    "id": "tooltip-1",
                    "target": {"Element": "a"},
                    "kind": {"Tooltip": {"text": "details"}}
                },
                {
                    "id": "stereotype-1",
                    "target": {"Element": "a"},
                    "kind": {"Stereotype": {"name": "service"}}
                },
                {
                    "id": "tag-1",
                    "target": "Diagram",
                    "kind": {"Tag": {"name": "owner", "value": "platform"}}
                }
            ])
        );
        assert_eq!(
            serde_json::from_str::<IrDocument>(&serialized).unwrap(),
            document
        );
    }

    #[test]
    fn document_rejects_unknown_versions_and_missing_required_fields() {
        let fixture: serde_json::Value = serde_json::from_str(EMPTY_GRAPH_DOCUMENT_V4).unwrap();

        for version in [0, 1, 5] {
            let mut value = fixture.clone();
            value["schema_version"] = json!(version);
            assert!(serde_json::from_value::<IrDocument>(value).is_err());
        }

        for field in [
            "schema_version",
            "metadata",
            "diagram",
            "annotations",
            "extensions",
        ] {
            let mut value = fixture.clone();
            value.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<IrDocument>(value).is_err(),
                "missing document field `{field}` must be rejected"
            );
        }

        for field in ["name", "title", "description", "accessibility"] {
            let mut value = fixture.clone();
            value["metadata"].as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<IrDocument>(value).is_err(),
                "missing metadata field `{field}` must be rejected"
            );
        }

        for field in ["title", "description"] {
            let mut value = fixture.clone();
            value["metadata"]["accessibility"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                serde_json::from_value::<IrDocument>(value).is_err(),
                "missing accessibility field `{field}` must be rejected"
            );
        }
    }

    #[test]
    fn v4_rejects_nested_unknown_fields_and_missing_tag_value() {
        let fixture: serde_json::Value = serde_json::from_str(EMPTY_GRAPH_DOCUMENT_V4).unwrap();
        let with_tag = |mut value: serde_json::Value| {
            value["annotations"] = json!([{
                "id": "tag-1",
                "target": "Diagram",
                "kind": {"Tag": {"name": "owner", "value": null}}
            }]);
            value
        };
        assert!(serde_json::from_value::<IrDocument>(with_tag(fixture.clone())).is_ok());

        let mut missing_value = with_tag(fixture.clone());
        missing_value["annotations"][0]["kind"]["Tag"]
            .as_object_mut()
            .unwrap()
            .remove("value");
        assert!(serde_json::from_value::<IrDocument>(missing_value).is_err());

        let mut unknown_metadata = fixture.clone();
        unknown_metadata["metadata"]["extra"] = json!(true);
        assert!(serde_json::from_value::<IrDocument>(unknown_metadata).is_err());

        let mut unknown_accessibility = fixture.clone();
        unknown_accessibility["metadata"]["accessibility"]["extra"] = json!(true);
        assert!(serde_json::from_value::<IrDocument>(unknown_accessibility).is_err());

        let mut unknown_annotation = with_tag(fixture.clone());
        unknown_annotation["annotations"][0]["extra"] = json!(true);
        assert!(serde_json::from_value::<IrDocument>(unknown_annotation).is_err());

        let mut unknown_kind_payload = with_tag(fixture);
        unknown_kind_payload["annotations"][0]["kind"]["Tag"]["extra"] = json!(true);
        assert!(serde_json::from_value::<IrDocument>(unknown_kind_payload).is_err());
    }

    #[test]
    fn extensions_serialize_in_canonical_key_order() {
        fn extensions(reverse: bool) -> Extensions {
            let entries = [
                (
                    "alpha".to_string(),
                    vec![
                        ("z".to_string(), json!({"b": 2, "a": 1})),
                        ("a".to_string(), json!([3, {"y": false, "x": null}])),
                    ],
                ),
                (
                    "zeta".to_string(),
                    vec![("nested".to_string(), json!({"items": [2, 1]}))],
                ),
            ];

            let mut namespaces = BTreeMap::new();
            let indices: &[usize] = if reverse { &[1, 0] } else { &[0, 1] };
            for &index in indices {
                let (namespace, values) = &entries[index];
                let mut fields = BTreeMap::new();
                let field_indices: Vec<usize> = if reverse {
                    (0..values.len()).rev().collect()
                } else {
                    (0..values.len()).collect()
                };
                for field_index in field_indices {
                    let (key, value) = &values[field_index];
                    fields.insert(key.clone(), value.clone());
                }
                namespaces.insert(namespace.clone(), fields);
            }
            Extensions(namespaces)
        }

        let first = serde_json::to_string(&extensions(false)).unwrap();
        let second = serde_json::to_string(&extensions(true)).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first,
            r#"{"alpha":{"a":[3,{"x":null,"y":false}],"z":{"a":1,"b":2}},"zeta":{"nested":{"items":[2,1]}}}"#
        );
    }

    #[test]
    fn named_diagram_wire_representations_are_unchanged() {
        let mut graph = GraphDiagram::new(Direction::Down);
        graph.nodes.insert("a".into(), Node::new("a", "A"));
        graph.edges.push(Edge::new(
            "a",
            "a",
            Some("loop".to_string()),
            ArrowType::Triangle,
        ));

        let mut sequence = SequenceDiagram::new();
        sequence
            .participants
            .insert("a".into(), Participant::new("a", "Alice"));
        sequence.items.push(SequenceItem::Message(Message::new(
            "a",
            "a",
            Some("call".to_string()),
            LineStyle::Solid,
            ArrowType::Triangle,
        )));

        let mut state = StateDiagram::new();
        state
            .states
            .insert("idle".into(), State::new("idle", "Idle"));
        state.transitions.push(Transition::new(
            Endpoint::State("idle".into()),
            Endpoint::State("idle".into()),
            Some("stay".to_string()),
        ));

        let mut class = ClassDiagram::new(Direction::Down);
        class
            .classes
            .insert("Order".into(), ClassNode::new("Order", "Order"));
        class.relations.push(ClassRelation::new(
            "Order",
            "Order",
            EndMarker::None,
            EndMarker::OpenArrow,
            LineStyle::Solid,
            Some("self".to_string()),
            None,
            None,
        ));

        let mut er = ErDiagram::new();
        er.entities
            .insert("Order".into(), ErEntity::new("Order", "Order"));
        er.relations.push(ErRelation::new(
            "Order",
            "Order",
            EndMarker::ErOne,
            EndMarker::ErZeroOrMany,
            Some("contains".to_string()),
            LineStyle::Solid,
        ));

        let fixtures = [
            (
                Diagram::Graph(graph),
                r#"{"Graph":{"direction":"Down","nodes":{"a":{"id":"a","label":"A","kind":"Default"}},"edges":[{"from":"a","to":"a","label":"loop","arrow":"Triangle"}]}}"#,
            ),
            (
                Diagram::Sequence(sequence),
                r#"{"Sequence":{"participants":{"a":{"id":"a","label":"Alice"}},"items":[{"Message":{"from":"a","to":"a","label":"call","line":"Solid","arrow":"Triangle"}}]}}"#,
            ),
            (
                Diagram::State(state),
                r#"{"State":{"states":{"idle":{"id":"idle","label":"Idle"}},"transitions":[{"from":{"State":"idle"},"to":{"State":"idle"},"label":"stay"}]}}"#,
            ),
            (
                Diagram::Class(class),
                r#"{"Class":{"direction":"Down","classes":{"Order":{"id":"Order","name":"Order","stereotype":null,"attributes":[],"methods":[]}},"relations":[{"from":"Order","to":"Order","from_marker":"None","to_marker":"OpenArrow","line":"Solid","label":"self","from_mult":null,"to_mult":null}]}}"#,
            ),
            (
                Diagram::Er(er),
                r#"{"Er":{"entities":{"Order":{"id":"Order","name":"Order","attributes":[]}},"relations":[{"from":"Order","to":"Order","from_marker":"ErOne","to_marker":"ErZeroOrMany","label":"contains","line":"Solid"}]}}"#,
            ),
        ];

        for (diagram, fixture) in fixtures {
            assert_eq!(serde_json::to_string(&diagram).unwrap(), fixture);
            assert_eq!(serde_json::from_str::<Diagram>(fixture).unwrap(), diagram);
        }
    }

    #[test]
    fn right_direction_bare_diagram_wire_representation_is_unchanged() {
        let graph = Diagram::Graph(GraphDiagram::new(Direction::Right));
        let class = Diagram::Class(ClassDiagram::new(Direction::Right));

        assert_eq!(
            serde_json::to_string(&graph).unwrap(),
            r#"{"Graph":{"direction":"Right","nodes":{},"edges":[]}}"#
        );
        assert_eq!(
            serde_json::to_string(&class).unwrap(),
            r#"{"Class":{"direction":"Right","classes":{},"relations":[]}}"#
        );
    }
}
