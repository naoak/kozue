//! Semantic layout types: the mapping from semantic diagram elements to
//! their geometric positions in the laid-out scene.
//!
//! These types are produced by [`crate::layout_full`] alongside the [`Scene`]
//! and let downstream consumers (exchange exporters, hit-testing, etc.) know
//! *which rectangle / polyline corresponds to which node / edge*.

use kozue_ir::Rect;

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
    pub id: String,
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
    pub id: String,
}

impl GraphEndpoint {
    pub fn new(id: impl Into<String>) -> Self {
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
    /// Routing points of the edge polyline in scene coordinates, in source-to-target order.
    /// These are the clipped endpoints and any bend points through dummy nodes.
    pub route: Vec<Point>,
    /// Center of the edge label, if the edge has a label.
    pub label_anchor: Option<Point>,
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
}

// ---------------------------------------------------------------------------
// Sequence layout
// ---------------------------------------------------------------------------

/// Layout information for a single participant in a sequence diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ParticipantLayout {
    /// The participant's stable string ID.
    pub id: String,
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
    pub from: String,
    /// Receiver participant ID.
    pub to: String,
    /// Routing points of the message arrow in scene coordinates (source to tip).
    pub route: Vec<Point>,
    /// Center of the message label, if any.
    pub label_anchor: Option<Point>,
}

/// Semantic layout for a sequence diagram.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceLayout {
    /// Participants in declaration order.
    pub participants: Vec<ParticipantLayout>,
    /// Messages in item order (only `Message` items; other future item types are skipped).
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
    State(String),
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
    pub id: String,
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
}
