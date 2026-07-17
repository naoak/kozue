//! Semantic layout types: the mapping from semantic diagram elements to
//! their geometric positions in the laid-out scene.
//!
//! These types are produced by [`crate::layout_full`] alongside the [`Scene`]
//! and let downstream consumers (exchange exporters, hit-testing, etc.) know
//! *which rectangle / polyline corresponds to which node / edge*.

use kozue_ir::{ElementId, NodeKind, ParticipantKind, Rect};

/// A 2-D point in scene coordinates (pixels, same coordinate space as
/// [`Scene`](kozue_ir::Scene)).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

// ---------------------------------------------------------------------------
// Graph layout
// ---------------------------------------------------------------------------

/// Layout information for a single graph node.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct NodeLayout {
    /// The node's stable string ID (from [`GraphDiagram::nodes`](kozue_ir::GraphDiagram)).
    pub id: ElementId,
    /// The display label text drawn in the node box (the same string emitted as
    /// the Scene Text item). This is the label, not the ID: for `a: "入力"` it is
    /// `"入力"`.
    pub label: String,
    /// Shape semantics retained from the graph IR.
    pub kind: NodeKind,
    /// The bounding rectangle of the node box in scene coordinates.
    pub rect: Rect,
    /// The center of the text label (the anchor used when emitting the Text item).
    pub label_anchor: Point,
}

/// The identifier of an edge endpoint in a graph diagram.
/// Stable string IDs from [`GraphDiagram::nodes`](kozue_ir::GraphDiagram).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEndpoint {
    pub id: ElementId,
}

impl GraphEndpoint {
    pub fn new(id: impl Into<ElementId>) -> Self {
        GraphEndpoint { id: id.into() }
    }
}

/// Layout information for a single graph edge.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeLayout {
    /// Index into [`GraphDiagram::edges`](kozue_ir::GraphDiagram) (0-based, declaration order).
    pub index: usize,
    /// Source endpoint.
    pub from: GraphEndpoint,
    /// Target endpoint.
    pub to: GraphEndpoint,
    /// Arrowhead style of this edge (from [`Edge::arrow`](kozue_ir::Edge)). Exporters
    /// use this to distinguish a directed edge from an undirected one
    /// ([`ArrowType::None`](kozue_ir::ArrowType)).
    pub arrow: kozue_ir::ArrowType,
    /// Arrowhead style at the source end of this edge (from
    /// [`Edge::from_arrow`](kozue_ir::Edge)).
    pub from_arrow: kozue_ir::ArrowType,
    /// Dash pattern of this edge's line (from [`Edge::line`](kozue_ir::Edge)).
    pub line: kozue_ir::LineStyle,
    /// Stroke weight of this edge's line (from [`Edge::weight`](kozue_ir::Edge)).
    pub weight: kozue_ir::LineWeight,
    /// Compass side of the source node this edge attaches to (from
    /// [`Edge::from_port`](kozue_ir::Edge)). `None` = default boundary clipping.
    pub from_port: Option<kozue_ir::Port>,
    /// Compass side of the target node this edge attaches to (from
    /// [`Edge::to_port`](kozue_ir::Edge)).
    pub to_port: Option<kozue_ir::Port>,
    /// Routing points of the edge polyline in scene coordinates, in source-to-target order.
    /// These are the clipped endpoints and any bend points through dummy nodes.
    pub route: Vec<Point>,
    /// The edge's label text, if any (from [`Edge::label`](kozue_ir::Edge)).
    pub label: Option<String>,
    /// Center of the edge label, if the edge has a label.
    pub label_anchor: Option<Point>,
}

/// Layout information for a single container (subgraph) box.
///
/// Naive M3a3 layout: a container is drawn as a bounding box behind its
/// members, computed *after* node placement (node placement and edge routing
/// are unaffected by containers). Real containment-aware layout is M4.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerLayout {
    /// The container's stable string ID (from
    /// [`Container::id`](kozue_ir::Container)).
    pub id: ElementId,
    pub label: Option<String>,
    /// The bounding rectangle of the container box in scene coordinates.
    pub rect: Rect,
    /// Top-left anchor of the label text, if the container has a label.
    pub label_anchor: Option<Point>,
    /// Direct member node ids (not nested-container members), in declaration order.
    pub members: Vec<ElementId>,
    /// Direct child container ids, in declaration order.
    pub children: Vec<ElementId>,
}

/// Semantic layout for a graph diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct GraphLayout {
    /// Nodes in declaration order (matches [`GraphDiagram::nodes`](kozue_ir::GraphDiagram)
    /// insertion order).
    pub nodes: Vec<NodeLayout>,
    /// Edges in declaration order (matches [`GraphDiagram::edges`](kozue_ir::GraphDiagram)).
    pub edges: Vec<EdgeLayout>,
    /// Containers, pre-order flattened (root, then each child recursively, in
    /// declaration order) so exchange exporters can iterate a flat list while
    /// still recovering the tree via `children`. Empty for a flat graph.
    pub containers: Vec<ContainerLayout>,
}

// ---------------------------------------------------------------------------
// Sequence layout
// ---------------------------------------------------------------------------

/// Layout information for a single participant in a sequence diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ParticipantLayout {
    /// The participant's stable string ID.
    pub id: ElementId,
    /// The display label text drawn in the header box (the same string emitted as
    /// the Scene Text item). This is the label, not the ID: for `participant a: "Alice"`
    /// it is `"Alice"`.
    pub label: String,
    /// The visual kind of this participant (from [`Participant::kind`](kozue_ir::Participant)).
    pub kind: ParticipantKind,
    /// The bounding box of the participant's header box.
    pub header_rect: Rect,
    /// The x-coordinate of the participant's lifeline (center of the column).
    pub lifeline_x: f64,
    /// The y-coordinate where the lifeline starts (bottom of the header box).
    pub lifeline_y0: f64,
    /// The y-coordinate where the lifeline ends.
    pub lifeline_y1: f64,
}

/// Layout information for a single message in a sequence diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct MessageLayout {
    /// Index into [`SequenceDiagram::items`](kozue_ir::SequenceDiagram) (0-based).
    pub index: usize,
    /// Sender participant ID.
    pub from: ElementId,
    /// Receiver participant ID.
    pub to: ElementId,
    /// Routing points of the message arrow in scene coordinates (source to tip).
    pub route: Vec<Point>,
    /// Line style of the message (from [`Message::line`](kozue_ir::Message)). Exporters
    /// use this to distinguish a solid call from a dashed reply.
    pub line: kozue_ir::LineStyle,
    /// Arrowhead style of the message (from [`Message::arrow`](kozue_ir::Message)).
    pub arrow: kozue_ir::ArrowType,
    /// The message's label text, if any (from [`Message::label`](kozue_ir::Message)).
    pub label: Option<String>,
    /// Center of the message label, if any.
    pub label_anchor: Option<Point>,
}

/// Semantic layout for a sequence diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceLayout {
    /// Participants in declaration order.
    pub participants: Vec<ParticipantLayout>,
    /// Messages in item order. Unsupported future item types are rejected by layout.
    pub messages: Vec<MessageLayout>,
}

// ---------------------------------------------------------------------------
// State layout
// ---------------------------------------------------------------------------

/// An endpoint in a state diagram transition.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateEndpointId {
    /// A named state; holds the state's stable string ID.
    State(ElementId),
    /// The synthetic initial pseudostate (filled circle).
    Initial,
    /// The synthetic final pseudostate (ringed circle).
    Final,
}

/// Layout information for a single state in a state diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct StateNodeLayout {
    /// The state's stable string ID. Includes both states declared explicitly in
    /// [`StateDiagram::states`](kozue_ir::StateDiagram) and states auto-declared by
    /// first appearance in a transition endpoint.
    pub id: ElementId,
    /// The display label text drawn in the state box (the same string emitted as
    /// the Scene Text item). For `state idle: "Idle"` it is `"Idle"`; for an
    /// auto-declared state the label defaults to the ID.
    pub label: String,
    /// The bounding rectangle of the state box in scene coordinates.
    pub rect: Rect,
    /// The center of the text label.
    pub label_anchor: Point,
}

/// Layout information for the initial pseudostate.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct InitialLayout {
    /// Center of the filled circle in scene coordinates.
    pub center: Point,
    /// Radius of the circle.
    pub radius: f64,
}

/// Layout information for the final pseudostate.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct FinalLayout {
    /// Center of the final-state rings in scene coordinates.
    pub center: Point,
    /// Radius of the inner filled circle.
    pub inner_radius: f64,
    /// Radius of the outer ring.
    pub outer_radius: f64,
}

/// Layout information for a single transition in a state diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionLayout {
    /// Index into [`StateDiagram::transitions`](kozue_ir::StateDiagram) (0-based).
    pub index: usize,
    /// Source endpoint.
    pub from: StateEndpointId,
    /// Target endpoint.
    pub to: StateEndpointId,
    /// The transition's label text, if any (from [`Transition::label`](kozue_ir::Transition)).
    pub label: Option<String>,
    /// Routing points of the transition polyline in scene coordinates.
    pub route: Vec<Point>,
    /// Center of the transition label, if any.
    pub label_anchor: Option<Point>,
}

/// Semantic layout for a state diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct StateLayout {
    /// Named states in declaration order.
    pub states: Vec<StateNodeLayout>,
    /// Initial pseudostate, if present.
    pub initial: Option<InitialLayout>,
    /// Final pseudostate, if present.
    pub final_state: Option<FinalLayout>,
    /// Transitions in declaration order (matches [`StateDiagram::transitions`](kozue_ir::StateDiagram)).
    pub transitions: Vec<TransitionLayout>,
}

// ---------------------------------------------------------------------------
// Class / ER layout
// ---------------------------------------------------------------------------

/// One horizontally-divided section of a [`CompartmentBox`] (e.g. the
/// attribute or method compartment of a class, or the column list of an ER
/// entity).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct Compartment {
    /// Y-coordinate (scene space) of the section's top divider line.
    pub top_y: f64,
    /// Pre-formatted display rows, top to bottom.
    pub rows: Vec<String>,
}

/// Layout information for a single class/interface (or ER entity) box.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct CompartmentBox {
    /// The stable string ID (from [`ClassDiagram::classes`](kozue_ir::ClassDiagram)
    /// or [`ErDiagram::entities`](kozue_ir::ErDiagram)).
    pub id: ElementId,
    /// The bounding rectangle of the whole box (title + all compartments).
    pub rect: Rect,
    pub title: String,
    pub stereotype: Option<String>,
    /// Compartments in top-to-bottom order. Empty sections are omitted.
    pub compartments: Vec<Compartment>,
}

/// Layout information for a single relation line between two boxes.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct RelationLayout {
    /// Index into the diagram's relation list (0-based, declaration order).
    pub index: usize,
    /// Source box ID.
    pub from: ElementId,
    /// Target box ID.
    pub to: ElementId,
    /// Routing points of the connecting line (from -> to order), in scene
    /// coordinates. Endpoints are clipped to the box borders but **not**
    /// shortened for the end markers: `points[0]` / `points[last]` sit exactly
    /// on the border where the marker tip attaches. A consumer that draws its
    /// own markers should retract the line by the marker's depth itself.
    pub points: Vec<(f64, f64)>,
    pub from_marker: kozue_ir::EndMarker,
    pub to_marker: kozue_ir::EndMarker,
    pub line: kozue_ir::LineStyle,
    pub label: Option<String>,
    pub from_mult: Option<String>,
    pub to_mult: Option<String>,
}

/// Semantic layout for a class diagram (boxes + relation lines).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ClassLayout {
    pub width: f64,
    pub height: f64,
    /// Boxes in declaration order.
    pub boxes: Vec<CompartmentBox>,
    /// Relations in declaration order.
    pub relations: Vec<RelationLayout>,
}

/// Semantic layout for an ER diagram. Structurally identical to
/// [`ClassLayout`] (entities become [`CompartmentBox`]es with a single
/// "columns" compartment); kept as a distinct name so `SemanticLayout::Er`
/// reads clearly at call sites.
pub type ErLayout = ClassLayout;

// ---------------------------------------------------------------------------
// Top-level enum
// ---------------------------------------------------------------------------

/// The semantic-to-geometry mapping for a laid-out diagram.
///
/// Produced by [`crate::layout_full`] alongside the [`Scene`](kozue_ir::Scene).
/// Lets consumers (exchange exporters, editors, etc.) find the geometric position
/// of any semantic element without re-running layout.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticLayout {
    Graph(GraphLayout),
    Sequence(SequenceLayout),
    State(StateLayout),
    Class(ClassLayout),
    Er(ErLayout),
}
