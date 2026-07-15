//! IR layers for kozue.
//!
//! - Semantic IR: [`Diagram`] and its variants describe *what* the diagram is.
//! - Scene IR: [`Scene`] and [`SceneItem`] describe *drawing primitives* only,
//!   already laid out. The renderer only ever sees the Scene IR.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Semantic IR
// ---------------------------------------------------------------------------

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
}

/// A graph diagram: a set of nodes and directed edges between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphDiagram {
    pub direction: Direction,
    /// Nodes keyed by their stable string ID. Iteration order is insertion
    /// (declaration) order, which the layout relies on for determinism.
    pub nodes: IndexMap<String, Node>,
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
    pub participants: IndexMap<String, Participant>,
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
    pub id: String,
    pub label: String,
}

impl Participant {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
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
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub line: LineStyle,
    pub arrow: ArrowType,
}

impl Message {
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
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
}

/// A node with a stable ID, a display label, and a kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub kind: NodeKind,
}

impl Node {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Node {
            id: id.into(),
            label: label.into(),
            kind: NodeKind::Default,
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
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub arrow: ArrowType,
}

impl Edge {
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
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
    pub states: IndexMap<String, State>,
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
    pub id: String,
    pub label: String,
}

impl State {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
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
    State(String),
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
    pub classes: IndexMap<String, ClassNode>,
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
    pub id: String,
    pub name: String,
    /// `"interface"` / `"abstract"` / `"enumeration"` (from `<<...>>` annotations).
    pub stereotype: Option<String>,
    /// Pre-formatted display lines, e.g. `"+name: String"`.
    pub attributes: Vec<String>,
    /// Pre-formatted display lines, e.g. `"+getName(): String"`.
    pub methods: Vec<String>,
}

impl ClassNode {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
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
    pub from: String,
    pub to: String,
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
        from: impl Into<String>,
        to: impl Into<String>,
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
    pub entities: IndexMap<String, ErEntity>,
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
    pub id: String,
    pub name: String,
    pub attributes: Vec<ErAttribute>,
}

impl ErEntity {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
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
    pub from: String,
    pub to: String,
    pub from_marker: EndMarker,
    pub to_marker: EndMarker,
    pub label: Option<String>,
    /// Solid = identifying relationship, Dashed = non-identifying.
    pub line: LineStyle,
}

impl ErRelation {
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
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
