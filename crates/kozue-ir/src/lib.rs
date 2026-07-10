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
