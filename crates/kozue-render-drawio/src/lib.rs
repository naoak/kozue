//! Deterministic draw.io (mxGraph XML / mxfile) exporter for the SemanticLayout.
//!
//! ## Rationale
//!
//! The draw.io format (`mxfile` / mxGraph XML) is a widely used interchange
//! format for editable vector diagrams. Unlike the raster PNG or terminal
//! renderers — which read only the flat [`Scene`](kozue_ir::Scene) IR — this
//! exporter reads the **semantic** layout produced by
//! [`kozue_layout::layout_full`] so that each mxCell maps to a meaningful
//! diagram element (node, edge, pseudostate, etc.) rather than to a raw
//! drawing primitive.
//!
//! ## Determinism
//!
//! Output is byte-identical for the same input:
//! - No `HashMap` anywhere; all collections use `Vec` (iteration order = layout order).
//! - Cell IDs are deterministic: vertices `n{i}`, edges `e{i}`, pseudostates
//!   `initial` / `final`.
//! - The diagram `id` attribute is the fixed string `"kozue"`.
//! - Float values are formatted with exactly 2 decimal places.
//! - Attribute order and indentation are fixed.
//!
//! ## Coordinate space
//!
//! [`SemanticLayout`] coordinates use the Scene coordinate system: origin at
//! (0, 0), y-axis pointing down. This matches draw.io's coordinate system
//! directly. A fixed 20 px margin is added on the exporter side (matching the
//! SVG / PNG renderers).
//!
//! ## Supported diagram types
//!
//! - [`SemanticLayout::Graph`] — each node becomes a rounded-rectangle vertex;
//!   each edge becomes a connector with waypoints.
//! - [`SemanticLayout::State`] — each named state becomes a rounded-rectangle
//!   vertex; the initial pseudostate becomes a filled ellipse; the final
//!   pseudostate becomes a double-ellipse. Transitions become connectors.
//!
//! [`SemanticLayout::Sequence`] and any future variants return
//! [`RenderError::UnsupportedDiagram`] rather than silently dropping data.

use kozue_ir::ArrowType;
use kozue_layout::semantic::{GraphLayout, SemanticLayout, StateEndpointId, StateLayout};

const MARGIN: f64 = 20.0;

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

/// An error that prevents a [`SemanticLayout`] from being exported to draw.io.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    /// The diagram type is not supported by this exporter (e.g. sequence
    /// diagrams). Returns an explicit error instead of silently dropping data.
    UnsupportedDiagram {
        /// Human-readable description of the unsupported variant.
        kind: &'static str,
    },
    /// A graph edge references a node ID that is not present in the layout.
    /// Silently dropping dangling edges would produce misleading output.
    DanglingEdge {
        /// The missing node ID.
        node_id: String,
    },
    /// A state transition references an endpoint that cannot be resolved to a
    /// cell. This covers unknown `StateEndpointId` variants added in the future
    /// (the type is `#[non_exhaustive]`).
    UnknownEndpoint {
        /// Human-readable description of the unresolved endpoint.
        description: String,
    },
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::UnsupportedDiagram { kind } => {
                write!(f, "draw.io export does not support {kind} diagrams")
            }
            RenderError::DanglingEdge { node_id } => {
                write!(
                    f,
                    "draw.io export: edge references unknown node \"{node_id}\""
                )
            }
            RenderError::UnknownEndpoint { description } => {
                write!(
                    f,
                    "draw.io export: cannot resolve transition endpoint: {description}"
                )
            }
        }
    }
}

impl std::error::Error for RenderError {}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Export a [`SemanticLayout`] to a draw.io (`mxfile`) XML string.
///
/// Returns byte-identical output for the same input on any target (see module
/// docs for the determinism guarantees).
///
/// # Errors
///
/// Returns [`RenderError::UnsupportedDiagram`] for sequence diagrams and any
/// future layout variants that have no mxGraph representation yet.
/// Returns [`RenderError::DanglingEdge`] if a graph edge references an unknown
/// node ID.
/// Returns [`RenderError::UnknownEndpoint`] if a state transition endpoint
/// cannot be resolved.
pub fn render(layout: &SemanticLayout) -> Result<String, RenderError> {
    match layout {
        SemanticLayout::Graph(g) => render_graph(g),
        SemanticLayout::State(s) => render_state(s),
        SemanticLayout::Sequence(_) => Err(RenderError::UnsupportedDiagram { kind: "sequence" }),
        _ => Err(RenderError::UnsupportedDiagram { kind: "unknown" }),
    }
}

// ---------------------------------------------------------------------------
// Float formatting
// ---------------------------------------------------------------------------

/// Format a float to exactly 2 decimal places (e.g. `"20.00"`, `"123.45"`).
fn f(v: f64) -> String {
    format!("{:.2}", v)
}

// ---------------------------------------------------------------------------
// XML escaping
// ---------------------------------------------------------------------------

/// Escape `< > & "` for use in XML attribute values and text content.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// mxfile skeleton helpers
// ---------------------------------------------------------------------------

fn mxfile_header() -> String {
    String::from(
        "<mxfile>\n\
         \x20 <diagram id=\"kozue\" name=\"Page-1\">\n\
         \x20   <mxGraphModel dx=\"1422\" dy=\"762\" grid=\"1\" gridSize=\"10\" \
         guides=\"1\" tooltips=\"1\" connect=\"1\" arrows=\"1\" fold=\"1\" \
         page=\"1\" pageScale=\"1\" pageWidth=\"1169\" pageHeight=\"827\" \
         math=\"0\" shadow=\"0\">\n\
         \x20     <root>\n\
         \x20       <mxCell id=\"0\"/>\n\
         \x20       <mxCell id=\"1\" parent=\"0\"/>\n",
    )
}

fn mxfile_footer() -> &'static str {
    "      </root>\n\
     \x20   </mxGraphModel>\n\
     \x20 </diagram>\n\
     </mxfile>\n"
}

// ---------------------------------------------------------------------------
// Waypoint helper
// ---------------------------------------------------------------------------

/// Emit an `<Array as="points">` element for interior waypoints.
///
/// `route[1..len-1]` are the interior points (excluding source and target
/// clip points which draw.io positions automatically).
fn waypoints_xml(route: &[kozue_layout::semantic::Point]) -> String {
    let len = route.len();
    if len <= 2 {
        return String::new();
    }
    let interior = &route[1..len - 1];
    let mut s = String::from("\n            <Array as=\"points\">");
    for pt in interior {
        s.push_str(&format!(
            "\n              <mxPoint x=\"{}\" y=\"{}\"/>",
            f(pt.x + MARGIN),
            f(pt.y + MARGIN)
        ));
    }
    s.push_str("\n            </Array>");
    s
}

// ---------------------------------------------------------------------------
// Graph diagram renderer
// ---------------------------------------------------------------------------

fn render_graph(g: &GraphLayout) -> Result<String, RenderError> {
    let mut out = mxfile_header();

    // Vertices — use label (display text), not id.
    for (i, node) in g.nodes.iter().enumerate() {
        let r = &node.rect;
        out.push_str(&format!(
            "        <mxCell id=\"n{i}\" value=\"{}\" style=\"rounded=1;whiteSpace=wrap;html=1;\" \
             vertex=\"1\" parent=\"1\">\n\
             \x20         <mxGeometry x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" as=\"geometry\"/>\n\
             \x20       </mxCell>\n",
            xml_escape(&node.label),
            f(r.x + MARGIN),
            f(r.y + MARGIN),
            f(r.width),
            f(r.height),
        ));
    }

    // Build node id -> index lookup (Vec-based, deterministic).
    // Returns RenderError::DanglingEdge for unknown IDs instead of silently
    // dropping source/target attributes.
    let find_node_idx = |id: &str| -> Option<usize> { g.nodes.iter().position(|n| n.id == id) };

    // Edges
    for (i, edge) in g.edges.iter().enumerate() {
        let src_idx = find_node_idx(&edge.from.id).ok_or_else(|| RenderError::DanglingEdge {
            node_id: edge.from.id.clone(),
        })?;
        let tgt_idx = find_node_idx(&edge.to.id).ok_or_else(|| RenderError::DanglingEdge {
            node_id: edge.to.id.clone(),
        })?;

        let src_attr = format!(" source=\"n{src_idx}\"");
        let tgt_attr = format!(" target=\"n{tgt_idx}\"");

        // Undirected edge: append endArrow=none; to the style.
        let style = if edge.arrow == ArrowType::None {
            "edgeStyle=orthogonalEdgeStyle;endArrow=none;"
        } else {
            "edgeStyle=orthogonalEdgeStyle;"
        };

        let label_value = xml_escape(edge.label.as_deref().unwrap_or(""));
        let wp = waypoints_xml(&edge.route);
        let has_children = !wp.is_empty();

        if has_children {
            out.push_str(&format!(
                "        <mxCell id=\"e{i}\" value=\"{label_value}\" style=\"{style}\" \
                 edge=\"1\"{src_attr}{tgt_attr} parent=\"1\">\n\
                 \x20         <mxGeometry relative=\"1\" as=\"geometry\">{wp}\n\
                 \x20         </mxGeometry>\n\
                 \x20       </mxCell>\n",
            ));
        } else {
            out.push_str(&format!(
                "        <mxCell id=\"e{i}\" value=\"{label_value}\" style=\"{style}\" \
                 edge=\"1\"{src_attr}{tgt_attr} parent=\"1\">\n\
                 \x20         <mxGeometry relative=\"1\" as=\"geometry\"/>\n\
                 \x20       </mxCell>\n",
            ));
        }
    }

    out.push_str(mxfile_footer());
    Ok(out)
}

// ---------------------------------------------------------------------------
// State diagram renderer
// ---------------------------------------------------------------------------

fn render_state(s: &StateLayout) -> Result<String, RenderError> {
    let mut out = mxfile_header();

    // Named state vertices — use label (display text), not id.
    for (i, state) in s.states.iter().enumerate() {
        let r = &state.rect;
        out.push_str(&format!(
            "        <mxCell id=\"n{i}\" value=\"{}\" style=\"rounded=1;whiteSpace=wrap;html=1;\" \
             vertex=\"1\" parent=\"1\">\n\
             \x20         <mxGeometry x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" as=\"geometry\"/>\n\
             \x20       </mxCell>\n",
            xml_escape(&state.label),
            f(r.x + MARGIN),
            f(r.y + MARGIN),
            f(r.width),
            f(r.height),
        ));
    }

    // Initial pseudostate (filled ellipse)
    if let Some(init) = &s.initial {
        let cx = init.center.x + MARGIN;
        let cy = init.center.y + MARGIN;
        let r = init.radius;
        out.push_str(&format!(
            "        <mxCell id=\"initial\" value=\"\" \
             style=\"ellipse;fillColor=#000000;strokeColor=#000000;\" \
             vertex=\"1\" parent=\"1\">\n\
             \x20         <mxGeometry x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" as=\"geometry\"/>\n\
             \x20       </mxCell>\n",
            f(cx - r),
            f(cy - r),
            f(r * 2.0),
            f(r * 2.0),
        ));
    }

    // Final pseudostate (double ellipse)
    if let Some(fin) = &s.final_state {
        let cx = fin.center.x + MARGIN;
        let cy = fin.center.y + MARGIN;
        let r = fin.outer_radius;
        out.push_str(&format!(
            "        <mxCell id=\"final\" value=\"\" \
             style=\"shape=doubleEllipse;fillColor=#000000;strokeColor=#000000;\" \
             vertex=\"1\" parent=\"1\">\n\
             \x20         <mxGeometry x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" as=\"geometry\"/>\n\
             \x20       </mxCell>\n",
            f(cx - r),
            f(cy - r),
            f(r * 2.0),
            f(r * 2.0),
        ));
    }

    // Build state id -> cell id lookup (Vec-based, deterministic).
    // Returns RenderError::UnknownEndpoint for unknown StateEndpointId variants
    // instead of silently producing a connector with no source/target.
    let state_cell_id = |ep: &StateEndpointId| -> Result<String, RenderError> {
        match ep {
            StateEndpointId::State(id) => s
                .states
                .iter()
                .position(|st| st.id == *id)
                .map(|i| format!("n{i}"))
                .ok_or_else(|| RenderError::UnknownEndpoint {
                    description: format!("state \"{id}\" not found in layout"),
                }),
            StateEndpointId::Initial => {
                if s.initial.is_some() {
                    Ok("initial".to_string())
                } else {
                    Err(RenderError::UnknownEndpoint {
                        description: "Initial pseudostate referenced but not present in layout"
                            .to_string(),
                    })
                }
            }
            StateEndpointId::Final => {
                if s.final_state.is_some() {
                    Ok("final".to_string())
                } else {
                    Err(RenderError::UnknownEndpoint {
                        description: "Final pseudostate referenced but not present in layout"
                            .to_string(),
                    })
                }
            }
            // Any future non_exhaustive variant -- refuse rather than silently
            // produce a connector with missing source or target.
            _ => Err(RenderError::UnknownEndpoint {
                description: format!("unrecognised StateEndpointId variant: {ep:?}"),
            }),
        }
    };

    // Transition edges
    for (i, tr) in s.transitions.iter().enumerate() {
        let src_cell = state_cell_id(&tr.from)?;
        let tgt_cell = state_cell_id(&tr.to)?;

        let src_attr = format!(" source=\"{src_cell}\"");
        let tgt_attr = format!(" target=\"{tgt_cell}\"");

        let label_value = xml_escape(tr.label.as_deref().unwrap_or(""));

        // Self-loop detection
        let is_self_loop = matches!(
            (&tr.from, &tr.to),
            (StateEndpointId::State(a), StateEndpointId::State(b)) if a == b
        );
        let style = if is_self_loop {
            "edgeStyle=orthogonalEdgeStyle;\
             exitX=1;exitY=0.5;exitDx=0;exitDy=0;\
             entryX=1;entryY=0;entryDx=0;entryDy=0;"
        } else {
            "edgeStyle=orthogonalEdgeStyle;"
        };

        let wp = waypoints_xml(&tr.route);
        let has_children = !wp.is_empty();

        if has_children {
            out.push_str(&format!(
                "        <mxCell id=\"e{i}\" value=\"{label_value}\" style=\"{style}\" \
                 edge=\"1\"{src_attr}{tgt_attr} parent=\"1\">\n\
                 \x20         <mxGeometry relative=\"1\" as=\"geometry\">{wp}\n\
                 \x20         </mxGeometry>\n\
                 \x20       </mxCell>\n",
            ));
        } else {
            out.push_str(&format!(
                "        <mxCell id=\"e{i}\" value=\"{label_value}\" style=\"{style}\" \
                 edge=\"1\"{src_attr}{tgt_attr} parent=\"1\">\n\
                 \x20         <mxGeometry relative=\"1\" as=\"geometry\"/>\n\
                 \x20       </mxCell>\n",
            ));
        }
    }

    out.push_str(mxfile_footer());
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a two-node graph layout via the real layout pipeline.
    fn graph_two_node_layout() -> SemanticLayout {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "Alpha"));
        g.nodes.insert("b".into(), Node::new("b", "Beta"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::Triangle));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        out.semantic
    }

    // Helper: build a basic state diagram layout via the real layout pipeline.
    fn state_basic_layout() -> SemanticLayout {
        use kozue_ir::{Diagram, Endpoint, State, StateDiagram, Transition};
        let mut sd = StateDiagram::new();
        sd.states.insert("idle".into(), State::new("idle", "Idle"));
        sd.states
            .insert("active".into(), State::new("active", "Active"));
        sd.transitions.push(Transition::new(
            Endpoint::Initial,
            Endpoint::State("idle".into()),
            None,
        ));
        sd.transitions.push(Transition::new(
            Endpoint::State("idle".into()),
            Endpoint::State("active".into()),
            Some("start".into()),
        ));
        sd.transitions.push(Transition::new(
            Endpoint::State("active".into()),
            Endpoint::Final,
            None,
        ));
        let out = kozue_layout::layout_full(&Diagram::State(sd)).expect("layout");
        out.semantic
    }

    // Helper: build a state diagram with a self-loop.
    fn state_self_loop_layout() -> SemanticLayout {
        use kozue_ir::{Diagram, Endpoint, State, StateDiagram, Transition};
        let mut sd = StateDiagram::new();
        sd.states.insert("s".into(), State::new("s", "S"));
        sd.transitions.push(Transition::new(
            Endpoint::Initial,
            Endpoint::State("s".into()),
            None,
        ));
        sd.transitions.push(Transition::new(
            Endpoint::State("s".into()),
            Endpoint::State("s".into()),
            Some("self".into()),
        ));
        let out = kozue_layout::layout_full(&Diagram::State(sd)).expect("layout");
        out.semantic
    }

    // Helper: sequence diagram layout.
    fn seq_layout() -> SemanticLayout {
        use kozue_ir::{
            ArrowType, Diagram, LineStyle, Message, Participant, SequenceDiagram, SequenceItem,
        };
        let mut seq = SequenceDiagram::new();
        seq.participants
            .insert("a".into(), Participant::new("a", "Alice"));
        seq.participants
            .insert("b".into(), Participant::new("b", "Bob"));
        seq.items.push(SequenceItem::Message(Message::new(
            "a",
            "b",
            Some("hi".into()),
            LineStyle::Solid,
            ArrowType::Triangle,
        )));
        let out = kozue_layout::layout_full(&Diagram::Sequence(seq)).expect("layout");
        out.semantic
    }

    // --- xml_escape ---

    #[test]
    fn xml_escape_passthrough() {
        assert_eq!(xml_escape("hello"), "hello");
    }

    #[test]
    fn xml_escape_special_chars() {
        assert_eq!(
            xml_escape("<b>bold & \"quotes\"</b>"),
            "&lt;b&gt;bold &amp; &quot;quotes&quot;&lt;/b&gt;"
        );
    }

    #[test]
    fn xml_escape_japanese_passthrough() {
        // CJK characters are not XML-special and must pass through unchanged.
        assert_eq!(xml_escape("入力"), "入力");
        assert_eq!(xml_escape("変換"), "変換");
    }

    // --- float formatting ---

    #[test]
    fn float_formatting_two_decimal_places() {
        assert_eq!(f(20.0), "20.00");
        assert_eq!(f(1.5), "1.50");
        assert_eq!(f(123.456), "123.46");
    }

    // --- graph rendering ---

    #[test]
    fn graph_render_produces_mxfile_skeleton() {
        let layout = graph_two_node_layout();
        let xml = render(&layout).expect("graph render");
        assert!(xml.starts_with("<mxfile>"), "must start with <mxfile>");
        assert!(xml.ends_with("</mxfile>\n"), "must end with </mxfile>");
        assert!(
            xml.contains("id=\"kozue\""),
            "diagram must have fixed id=kozue"
        );
        assert!(xml.contains("id=\"0\""), "root cell 0 must be present");
        assert!(xml.contains("id=\"1\""), "root cell 1 must be present");
        assert!(xml.contains("id=\"n0\""), "first node must be n0");
        assert!(xml.contains("id=\"n1\""), "second node must be n1");
        assert!(xml.contains("id=\"e0\""), "first edge must be e0");
        assert!(xml.contains("source=\"n0\""), "edge must have source");
        assert!(xml.contains("target=\"n1\""), "edge must have target");
    }

    #[test]
    fn graph_render_applies_margin() {
        let layout = graph_two_node_layout();
        let xml = render(&layout).expect("render");
        // All geometry x/y values must be > 0 (margin applied)
        // The first node starts at scene (0,0), so with margin it should be (20.00, 20.00).
        assert!(
            xml.contains("x=\"20.00\" y=\"20.00\""),
            "margin must offset first node: {xml}"
        );
    }

    #[test]
    fn graph_render_is_deterministic() {
        let layout = graph_two_node_layout();
        let xml1 = render(&layout).expect("render 1");
        let xml2 = render(&layout).expect("render 2");
        assert_eq!(xml1, xml2, "same input must produce byte-identical output");
    }

    #[test]
    fn graph_render_node_value_is_label_not_id() {
        // Node id="a", label="Alpha": the mxCell value must be "Alpha", not "a".
        let layout = graph_two_node_layout();
        let xml = render(&layout).expect("render");
        assert!(
            xml.contains("value=\"Alpha\""),
            "node value must be display label: {xml}"
        );
        assert!(
            xml.contains("value=\"Beta\""),
            "second node value must be display label: {xml}"
        );
    }

    #[test]
    fn graph_render_node_label_is_xml_escaped() {
        use kozue_ir::{Diagram, Direction, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        // Node with display label that contains XML-special characters.
        g.nodes
            .insert("x".into(), Node::new("x", "A < B & C \"quoted\""));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let xml = render(&out.semantic).expect("render");
        assert!(
            xml.contains("&lt;"),
            "< in node label must be escaped: {xml}"
        );
        assert!(
            xml.contains("&amp;"),
            "& in node label must be escaped: {xml}"
        );
        assert!(
            xml.contains("&quot;"),
            "\" in node label must be escaped: {xml}"
        );
        assert!(
            !xml.contains("\"A < B"),
            "raw special chars must not appear unescaped: {xml}"
        );
    }

    #[test]
    fn graph_render_node_label_japanese() {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "入力"));
        g.nodes.insert("b".into(), Node::new("b", "出力"));
        g.edges.push(Edge::new(
            "a",
            "b",
            Some("処理".into()),
            ArrowType::Triangle,
        ));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let xml = render(&out.semantic).expect("render");
        assert!(
            xml.contains("value=\"入力\""),
            "Japanese node label must appear: {xml}"
        );
        assert!(
            xml.contains("value=\"出力\""),
            "Japanese node label must appear: {xml}"
        );
        assert!(
            xml.contains("value=\"処理\""),
            "Japanese edge label must appear: {xml}"
        );
    }

    #[test]
    fn graph_render_edge_value_is_label() {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        g.edges.push(Edge::new(
            "a",
            "b",
            Some("myLabel".into()),
            ArrowType::Triangle,
        ));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let xml = render(&out.semantic).expect("render");
        assert!(
            xml.contains("value=\"myLabel\""),
            "edge value must be its label: {xml}"
        );
    }

    #[test]
    fn graph_render_undirected_edge_has_no_arrowhead_style() {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::None));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let xml = render(&out.semantic).expect("render");
        assert!(
            xml.contains("endArrow=none;"),
            "undirected edge must have endArrow=none: {xml}"
        );
    }

    #[test]
    fn graph_render_directed_edge_does_not_have_endarrow_none() {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::Triangle));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let xml = render(&out.semantic).expect("render");
        assert!(
            !xml.contains("endArrow=none"),
            "directed edge must not have endArrow=none: {xml}"
        );
    }

    // --- state rendering ---

    #[test]
    fn state_render_has_initial_and_final() {
        let layout = state_basic_layout();
        let xml = render(&layout).expect("state render");
        assert!(xml.contains("id=\"initial\""), "initial pseudostate cell");
        assert!(xml.contains("id=\"final\""), "final pseudostate cell");
        assert!(
            xml.contains("ellipse;fillColor=#000000"),
            "initial is filled ellipse"
        );
        assert!(
            xml.contains("shape=doubleEllipse"),
            "final is double ellipse"
        );
        assert!(
            xml.contains("source=\"initial\""),
            "transition from initial"
        );
        assert!(xml.contains("target=\"final\""), "transition to final");
        assert!(xml.contains("id=\"n0\""), "first named state is n0");
        assert!(xml.contains("id=\"n1\""), "second named state is n1");
    }

    #[test]
    fn state_render_node_value_is_label_not_id() {
        let layout = state_basic_layout();
        let xml = render(&layout).expect("state render");
        // state idle: "Idle" => value="Idle", not value="idle"
        assert!(
            xml.contains("value=\"Idle\""),
            "state value must be display label: {xml}"
        );
        assert!(
            xml.contains("value=\"Active\""),
            "state value must be display label: {xml}"
        );
        assert!(
            !xml.contains("value=\"idle\""),
            "raw state ID must not appear as value: {xml}"
        );
    }

    #[test]
    fn state_render_transition_value_is_label() {
        let layout = state_basic_layout();
        let xml = render(&layout).expect("state render");
        // idle -> active : "start" => transition value="start"
        assert!(
            xml.contains("value=\"start\""),
            "transition value must be its label: {xml}"
        );
    }

    #[test]
    fn state_render_self_loop_style() {
        let layout = state_self_loop_layout();
        let xml = render(&layout).expect("state render");
        assert!(xml.contains("exitX=1"), "self-loop must have exit style");
        assert!(xml.contains("entryX=1"), "self-loop must have entry style");
    }

    #[test]
    fn state_render_is_deterministic() {
        let layout = state_basic_layout();
        let xml1 = render(&layout).expect("render 1");
        let xml2 = render(&layout).expect("render 2");
        assert_eq!(xml1, xml2, "state render must be deterministic");
    }

    // --- unsupported variants ---

    #[test]
    fn sequence_diagram_returns_explicit_error() {
        let layout = seq_layout();
        let result = render(&layout);
        assert!(
            matches!(
                result,
                Err(RenderError::UnsupportedDiagram { kind: "sequence" })
            ),
            "sequence must return UnsupportedDiagram, got: {result:?}"
        );
    }

    #[test]
    fn render_error_display() {
        let e = RenderError::UnsupportedDiagram { kind: "sequence" };
        let msg = e.to_string();
        assert!(
            msg.contains("sequence"),
            "error message must mention the kind"
        );
    }

    #[test]
    fn dangling_edge_error_display() {
        let e = RenderError::DanglingEdge {
            node_id: "missing_node".to_string(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("missing_node"),
            "error must mention the node id: {msg}"
        );
    }

    #[test]
    fn unknown_endpoint_error_display() {
        let e = RenderError::UnknownEndpoint {
            description: "Initial pseudostate not present".to_string(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("Initial"),
            "error must mention the description: {msg}"
        );
    }

    // --- waypoints ---

    #[test]
    fn waypoints_xml_empty_for_two_point_route() {
        use kozue_layout::semantic::Point;
        let route = vec![Point::new(0.0, 0.0), Point::new(10.0, 10.0)];
        let s = waypoints_xml(&route);
        assert!(s.is_empty(), "two-point route must produce no waypoints");
    }

    #[test]
    fn waypoints_xml_emits_interior_points_with_margin() {
        use kozue_layout::semantic::Point;
        let route = vec![
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.0),
            Point::new(5.0, 10.0),
            Point::new(10.0, 10.0),
        ];
        let s = waypoints_xml(&route);
        assert!(
            s.contains("<Array as=\"points\">"),
            "must have Array element: {s}"
        );
        assert!(s.contains("<mxPoint"), "must have mxPoint elements: {s}");
        // Interior points are (5,0) and (5,10), with +20 margin -> (25,20) and (25,30)
        assert!(
            s.contains("x=\"25.00\" y=\"20.00\""),
            "first interior point with margin: {s}"
        );
        assert!(
            s.contains("x=\"25.00\" y=\"30.00\""),
            "second interior point with margin: {s}"
        );
    }

    // --- xml escape for node/edge values ---

    #[test]
    fn node_value_and_edge_value_are_xml_escaped() {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        // Node label with XML-special chars
        g.nodes.insert("p".into(), Node::new("p", "A < B & \"C\""));
        // Edge label with XML-special chars
        g.nodes.insert("q".into(), Node::new("q", "Normal"));
        g.edges.push(Edge::new(
            "p",
            "q",
            Some("x > y & z".into()),
            ArrowType::Triangle,
        ));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let xml = render(&out.semantic).expect("render");

        // Node value must be escaped
        assert!(
            xml.contains("value=\"A &lt; B &amp; &quot;C&quot;\""),
            "node label must be XML-escaped: {xml}"
        );
        // Edge value must be escaped
        assert!(
            xml.contains("value=\"x &gt; y &amp; z\""),
            "edge label must be XML-escaped: {xml}"
        );
    }

    #[test]
    fn node_value_japanese_and_special_chars_in_edge() {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "入力<データ>"));
        g.nodes.insert("b".into(), Node::new("b", "出力"));
        g.edges.push(Edge::new(
            "a",
            "b",
            Some("送信 & 受信".into()),
            ArrowType::Triangle,
        ));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let xml = render(&out.semantic).expect("render");
        assert!(
            xml.contains("value=\"入力&lt;データ&gt;\""),
            "Japanese + XML escape in node: {xml}"
        );
        assert!(
            xml.contains("value=\"送信 &amp; 受信\""),
            "Japanese + XML escape in edge: {xml}"
        );
    }
}
