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
//!   pseudostate becomes an outer stroked ring plus an inner filled dot (two
//!   concentric ellipses). Transitions become connectors.
//! - [`SemanticLayout::Sequence`] — each participant becomes a `umlLifeline`
//!   vertex (header box + dashed lifeline in one shape). Each message becomes a
//!   connector whose endpoints are pinned to a fractional height on the source
//!   and target lifelines via `exitY` / `entryY`, so the vertical (time) order
//!   survives while the horizontal connection follows when a participant is
//!   moved (see the M8c design notes). Self-messages become self-loop
//!   connectors with explicit fold waypoints.
//! - [`SemanticLayout::Class`] / [`SemanticLayout::Er`] — each
//!   [`CompartmentBox`] becomes a rounded-rectangle vertex whose HTML `value`
//!   lays out the title/stereotype followed by one `<hr>`-separated section
//!   per compartment. Each [`RelationLayout`] becomes a connector whose
//!   `startArrow`/`endArrow` (+ `Fill`) encode the UML/ER end markers (see
//!   [`drawio_arrow`]); multiplicities are emitted as child `edgeLabel`
//!   cells near each endpoint.
//!
//! Any future variants return [`RenderError::UnsupportedDiagram`] rather than
//! silently dropping data.

use kozue_ir::{ArrowType, EndMarker, LineStyle, LineWeight, NodeKind, ParticipantKind, Port};
use kozue_layout::semantic::{
    ClassLayout, CompartmentBox, GraphLayout, SemanticLayout, SequenceLayout, StateEndpointId,
    StateLayout,
};
use kozue_layout::ExportInput;

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
    /// A future graph node kind has no defined draw.io mapping.
    UnknownNodeKind { description: String },
    /// A future semantic enum variant has no defined export mapping.
    InvalidSemantic { description: String },
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
            RenderError::UnknownNodeKind { description } => {
                write!(f, "draw.io export: unknown graph node kind: {description}")
            }
            RenderError::InvalidSemantic { description } => {
                write!(f, "draw.io export: invalid semantic value: {description}")
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
/// Returns [`RenderError::UnsupportedDiagram`] for any future layout variants
/// that have no mxGraph representation yet.
/// Returns [`RenderError::DanglingEdge`] if a graph edge or sequence message
/// references an unknown node/participant ID.
/// Returns [`RenderError::UnknownEndpoint`] if a state transition endpoint
/// cannot be resolved.
pub fn render(layout: &SemanticLayout) -> Result<String, RenderError> {
    kozue_layout::validate_export_semantics(layout).map_err(|error| {
        RenderError::InvalidSemantic {
            description: error.to_string(),
        }
    })?;
    match layout {
        SemanticLayout::Graph(g) => render_graph(g),
        SemanticLayout::State(s) => render_state(s),
        SemanticLayout::Sequence(seq) => render_sequence(seq),
        SemanticLayout::Class(c) => render_class(c),
        SemanticLayout::Er(e) => render_er(e),
        _ => Err(RenderError::UnsupportedDiagram { kind: "unknown" }),
    }
}

/// Export a validated diagram/scene/semantic contract to draw.io XML.
pub fn render_export(input: &ExportInput<'_>) -> Result<String, RenderError> {
    render(input.semantic())
}

// ---------------------------------------------------------------------------
// Float formatting
// ---------------------------------------------------------------------------

/// Format a float to exactly 2 decimal places (e.g. `"20.00"`, `"123.45"`).
fn f(v: f64) -> String {
    format!("{:.2}", v)
}

/// Format a connection-point fraction (0.0–1.0) to exactly 6 decimal places.
///
/// Sequence message endpoints are pinned to a fractional height along a lifeline
/// via `exitY` / `entryY`. The coordinate formatter [`f`] (2 decimals) is far too
/// coarse here: on a 2000 px-tall lifeline, 2 decimals quantises to 20 px, which
/// would collapse adjacent messages onto the same row and destroy the time order.
/// Six decimals keep the error below 0.1 px even for very tall diagrams.
fn frac(v: f64) -> String {
    format!("{:.6}", v)
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

/// Build the mxCell `style` attribute for a graph edge.
///
/// Presentation attributes are appended in a fixed order so the common
/// (default-presentation) case stays byte-identical to the pre-M3a2b output:
/// `edgeStyle=orthogonalEdgeStyle;` alone, or with `endArrow=none;` for an
/// undirected edge. `from_arrow`/`line`/`weight` are only present (and only
/// checked) when they differ from their defaults — the caller has already
/// gone through [`kozue_layout::validate_export_semantics`], so future enum
/// variants never reach here.
#[allow(clippy::too_many_arguments)]
fn graph_edge_style(
    arrow: ArrowType,
    from_arrow: ArrowType,
    line: LineStyle,
    weight: LineWeight,
    from_port: Option<Port>,
    to_port: Option<Port>,
) -> Result<String, RenderError> {
    let mut style = String::from("edgeStyle=orthogonalEdgeStyle;");
    if arrow == ArrowType::None {
        style.push_str("endArrow=none;");
    }
    if from_arrow == ArrowType::Triangle {
        style.push_str("startArrow=classic;");
    }
    match line {
        LineStyle::Solid => {}
        LineStyle::Dashed => style.push_str("dashed=1;"),
        LineStyle::Dotted => style.push_str("dashed=1;dashPattern=1 4;"),
        _ => {
            return Err(RenderError::InvalidSemantic {
                description: format!("unsupported draw.io line style: {line:?}"),
            })
        }
    }
    match weight {
        LineWeight::Normal => {}
        LineWeight::Thick => style.push_str("strokeWidth=3;"),
        _ => {
            return Err(RenderError::InvalidSemantic {
                description: format!("unsupported draw.io line weight: {weight:?}"),
            })
        }
    }
    // Compass port attachment. Appended only when present, in a fixed order
    // (source exit, then target entry), so the default (both `None`) case
    // stays byte-identical to the pre-port output.
    if let Some(port) = from_port {
        style.push_str(&port_exit_entry("exit", port)?);
    }
    if let Some(port) = to_port {
        style.push_str(&port_exit_entry("entry", port)?);
    }
    Ok(style)
}

/// Map a compass [`Port`] to its draw.io `exitX/exitY` or `entryX/entryY`
/// fractional connection-point attributes, e.g. `exitX=1;exitY=0.5;exitDx=0;exitDy=0;`.
///
/// Fractions use the minimal decimal representation (`0`/`0.5`/`1`), matching
/// the existing state-diagram self-loop precedent rather than the 2-decimal
/// coordinate formatter [`f`]. Future `#[non_exhaustive]` [`Port`] variants are
/// explicit errors rather than a silent default.
fn port_exit_entry(prefix: &str, port: Port) -> Result<String, RenderError> {
    let (x, y) = match port {
        Port::North => ("0.5", "0"),
        Port::East => ("1", "0.5"),
        Port::South => ("0.5", "1"),
        Port::West => ("0", "0.5"),
        other => {
            return Err(RenderError::InvalidSemantic {
                description: format!("unsupported future port: {other:?}"),
            })
        }
    };
    Ok(format!(
        "{prefix}X={x};{prefix}Y={y};{prefix}Dx=0;{prefix}Dy=0;"
    ))
}

fn render_graph(g: &GraphLayout) -> Result<String, RenderError> {
    let mut out = mxfile_header();

    // Container backdrops — plain rectangles behind everything else, in
    // pre-order (matching `GraphLayout::containers`). This is a backdrop, not
    // an mxCell parent grouping: node cells still keep `parent="1"` with
    // absolute geometry, unchanged from before containers existed.
    for (j, c) in g.containers.iter().enumerate() {
        let r = &c.rect;
        out.push_str(&format!(
            "        <mxCell id=\"c{j}\" value=\"{}\" \
             style=\"rounded=0;dashed=1;fillColor=none;verticalAlign=top;align=left;\
             spacingLeft=6;spacingTop=4;html=1;\" vertex=\"1\" parent=\"1\">\n\
             \x20         <mxGeometry x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" as=\"geometry\"/>\n\
             \x20       </mxCell>\n",
            xml_escape(c.label.as_deref().unwrap_or("")),
            f(r.x + MARGIN),
            f(r.y + MARGIN),
            f(r.width),
            f(r.height),
        ));
    }

    // Vertices — use label (display text), not id.
    for (i, node) in g.nodes.iter().enumerate() {
        let r = &node.rect;
        let shape_style = match &node.kind {
            NodeKind::Default | NodeKind::RoundedRectangle => "rounded=1",
            NodeKind::Rectangle => "rounded=0",
            NodeKind::Circle => "ellipse",
            NodeKind::Diamond => "rhombus",
            kind => {
                return Err(RenderError::UnknownNodeKind {
                    description: format!("{kind:?}"),
                })
            }
        };
        out.push_str(&format!(
            "        <mxCell id=\"n{i}\" value=\"{}\" style=\"{shape_style};whiteSpace=wrap;html=1;\" \
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
    let find_node_idx =
        |id: &str| -> Option<usize> { g.nodes.iter().position(|n| n.id.as_str() == id) };

    // Edges
    for (i, edge) in g.edges.iter().enumerate() {
        let src_idx =
            find_node_idx(edge.from.id.as_str()).ok_or_else(|| RenderError::DanglingEdge {
                node_id: edge.from.id.to_string(),
            })?;
        let tgt_idx =
            find_node_idx(edge.to.id.as_str()).ok_or_else(|| RenderError::DanglingEdge {
                node_id: edge.to.id.to_string(),
            })?;

        let src_attr = format!(" source=\"n{src_idx}\"");
        let tgt_attr = format!(" target=\"n{tgt_idx}\"");

        let style = graph_edge_style(
            edge.arrow,
            edge.from_arrow,
            edge.line,
            edge.weight,
            edge.from_port,
            edge.to_port,
        )?;

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

    // Final pseudostate: an outer stroked ring plus an inner filled dot, matching
    // the SVG renderer. A single `shape=doubleEllipse;fillColor=#000000` renders
    // as a solid black blob in draw.io (both fills are black, so the ring gap is
    // invisible), so we emit two concentric ellipse cells instead. The outer ring
    // (`final`) is the connectable cell that transitions target; the inner dot
    // (`final_inner`) is decorative and drawn on top.
    if let Some(fin) = &s.final_state {
        let cx = fin.center.x + MARGIN;
        let cy = fin.center.y + MARGIN;
        let ro = fin.outer_radius;
        let ri = fin.inner_radius;
        out.push_str(&format!(
            "        <mxCell id=\"final\" value=\"\" \
             style=\"ellipse;fillColor=none;strokeColor=#000000;\" \
             vertex=\"1\" parent=\"1\">\n\
             \x20         <mxGeometry x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" as=\"geometry\"/>\n\
             \x20       </mxCell>\n",
            f(cx - ro),
            f(cy - ro),
            f(ro * 2.0),
            f(ro * 2.0),
        ));
        // The inner dot is a *child* of the outer ring (`parent="final"`) so the two
        // move together when edited; its geometry is in the ring's local coordinate
        // space (origin at the ring's top-left, i.e. `ro - ri` in from each side).
        out.push_str(&format!(
            "        <mxCell id=\"final_inner\" value=\"\" \
             style=\"ellipse;fillColor=#000000;strokeColor=#000000;\" \
             vertex=\"1\" parent=\"final\">\n\
             \x20         <mxGeometry x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" as=\"geometry\"/>\n\
             \x20       </mxCell>\n",
            f(ro - ri),
            f(ro - ri),
            f(ri * 2.0),
            f(ri * 2.0),
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
/// Build the draw.io `value` for a participant cell.
///
/// For non-Default kinds the stereotype is prepended as an italic line.
fn participant_cell_value(label: &str, kind: &ParticipantKind) -> String {
    match kind {
        ParticipantKind::Default => xml_escape(label),
        ParticipantKind::Actor => {
            format!("<i>«actor»</i><br>{}", xml_escape(label))
        }
        ParticipantKind::Boundary => {
            format!("<i>«boundary»</i><br>{}", xml_escape(label))
        }
        ParticipantKind::Control => {
            format!("<i>«control»</i><br>{}", xml_escape(label))
        }
        ParticipantKind::Entity => {
            format!("<i>«entity»</i><br>{}", xml_escape(label))
        }
        ParticipantKind::Database => {
            format!("<i>«database»</i><br>{}", xml_escape(label))
        }
        ParticipantKind::Collections => {
            format!("<i>«collections»</i><br>{}", xml_escape(label))
        }
        ParticipantKind::Queue => {
            format!("<i>«queue»</i><br>{}", xml_escape(label))
        }
        _ => xml_escape(label),
    }
}

// Sequence diagram renderer
// ---------------------------------------------------------------------------

fn render_sequence(s: &SequenceLayout) -> Result<String, RenderError> {
    let mut out = mxfile_header();

    // Participant lifelines — one umlLifeline vertex per participant (`n{i}`).
    // The vertex spans the *whole* column: its top is the header top and its
    // height reaches the bottom of the lifeline, so `exitY`/`entryY` fractions
    // (computed below) address a point on the dashed line.
    for (i, p) in s.participants.iter().enumerate() {
        let r = &p.header_rect;
        let height = p.lifeline_y1 - r.y;
        let value = participant_cell_value(&p.label, &p.kind);
        out.push_str(&format!(
            "        <mxCell id=\"n{i}\" value=\"{}\" \
             style=\"shape=umlLifeline;perimeter=lifelinePerimeter;size={};container=0;\
             collapsible=0;whiteSpace=wrap;html=1;\" vertex=\"1\" parent=\"1\">\n\
             \x20         <mxGeometry x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" as=\"geometry\"/>\n\
             \x20       </mxCell>\n",
            value,
            f(r.height),
            f(r.x + MARGIN),
            f(r.y + MARGIN),
            f(r.width),
            f(height),
        ));
    }

    // Participant id -> index lookup (Vec-based, deterministic). Returns
    // DanglingEdge for an unknown participant instead of dropping the message.
    let find_participant =
        |id: &str| -> Option<usize> { s.participants.iter().position(|p| p.id.as_str() == id) };

    // Messages — one connector per message (`e{i}`), pinned to fractional
    // heights on the source/target lifelines.
    for (i, m) in s.messages.iter().enumerate() {
        let src_idx =
            find_participant(m.from.as_str()).ok_or_else(|| RenderError::DanglingEdge {
                node_id: m.from.to_string(),
            })?;
        let tgt_idx = find_participant(m.to.as_str()).ok_or_else(|| RenderError::DanglingEdge {
            node_id: m.to.to_string(),
        })?;
        let src = &s.participants[src_idx];
        let tgt = &s.participants[tgt_idx];

        // Fraction of each vertex's full height. The denominator is the whole
        // vertex height (`lifeline_y1 - header_rect.y`), NOT the lifeline span,
        // because exitY/entryY are relative to the mxCell geometry, whose origin
        // is the header top.
        let y_from = m.route.first().map(|p| p.y).unwrap_or(src.lifeline_y0);
        let y_to = m.route.last().map(|p| p.y).unwrap_or(tgt.lifeline_y0);
        let exit_y = (y_from - src.header_rect.y) / (src.lifeline_y1 - src.header_rect.y);
        let entry_y = (y_to - tgt.header_rect.y) / (tgt.lifeline_y1 - tgt.header_rect.y);

        let dashed = if m.line == LineStyle::Dashed {
            "dashed=1;"
        } else {
            ""
        };
        // Undirected message (no arrowhead); directed messages use draw.io's
        // default arrowhead, matching the graph/state renderers.
        let end_arrow = if m.arrow == ArrowType::None {
            "endArrow=none;"
        } else {
            ""
        };

        let style = format!(
            "edgeStyle=none;html=1;{dashed}exitX=0.5;exitY={};exitDx=0;exitDy=0;exitPerimeter=0;\
             entryX=0.5;entryY={};entryDx=0;entryDy=0;entryPerimeter=0;{end_arrow}",
            frac(exit_y),
            frac(entry_y),
        );

        let label_value = xml_escape(m.label.as_deref().unwrap_or(""));
        let is_self = m.from == m.to;

        // A self-message routes right-down-left; its fold corners (route interior
        // points) are emitted as absolute waypoints. mxGraph translates a
        // self-loop's waypoints together with its vertex on move, so the loop
        // follows the participant.
        let wp = waypoints_xml(&m.route);

        // Label placement. A straight message keeps its label inline in the edge
        // `value`: draw.io centers it on the line and it follows on move. A
        // self-loop's inline value lags behind when the participant is dragged, so
        // its label is emitted as a child `edgeLabel` cell (the form draw.io itself
        // writes on label drag), whose position is recomputed from the edge path
        // and therefore follows the loop.
        let edge_value = if is_self { "" } else { &label_value };

        if wp.is_empty() {
            out.push_str(&format!(
                "        <mxCell id=\"e{i}\" value=\"{edge_value}\" style=\"{style}\" \
                 edge=\"1\" source=\"n{src_idx}\" target=\"n{tgt_idx}\" parent=\"1\">\n\
                 \x20         <mxGeometry relative=\"1\" as=\"geometry\"/>\n\
                 \x20       </mxCell>\n",
            ));
        } else {
            out.push_str(&format!(
                "        <mxCell id=\"e{i}\" value=\"{edge_value}\" style=\"{style}\" \
                 edge=\"1\" source=\"n{src_idx}\" target=\"n{tgt_idx}\" parent=\"1\">\n\
                 \x20         <mxGeometry relative=\"1\" as=\"geometry\">{wp}\n\
                 \x20         </mxGeometry>\n\
                 \x20       </mxCell>\n",
            ));
        }

        // Self-loop label child cell (only when the message has a label).
        if is_self && m.label.is_some() {
            out.push_str(&format!(
                "        <mxCell id=\"e{i}_label\" value=\"{label_value}\" \
                 style=\"edgeLabel;html=1;align=center;verticalAlign=middle;\" \
                 vertex=\"1\" connectable=\"0\" parent=\"e{i}\">\n\
                 \x20         <mxGeometry relative=\"1\" as=\"geometry\">\n\
                 \x20           <mxPoint as=\"offset\"/>\n\
                 \x20         </mxGeometry>\n\
                 \x20       </mxCell>\n",
            ));
        }
    }

    out.push_str(mxfile_footer());
    Ok(out)
}

// ---------------------------------------------------------------------------
// Class / ER diagram renderer
// ---------------------------------------------------------------------------

/// Emit an `<Array as="points">` element for a class/ER relation's interior
/// waypoints (tuple-based route, unlike [`waypoints_xml`]'s `Point`-based
/// graph/state/sequence routes).
fn waypoints_xml_tuples(route: &[(f64, f64)]) -> String {
    let len = route.len();
    if len <= 2 {
        return String::new();
    }
    let interior = &route[1..len - 1];
    let mut s = String::from("\n            <Array as=\"points\">");
    for &(x, y) in interior {
        s.push_str(&format!(
            "\n              <mxPoint x=\"{}\" y=\"{}\"/>",
            f(x + MARGIN),
            f(y + MARGIN)
        ));
    }
    s.push_str("\n            </Array>");
    s
}

/// Map an [`EndMarker`] to a draw.io `startArrow`/`endArrow` shape name and
/// whether it should be filled (`Fill=1`) or hollow (`Fill=0`).
///
/// | `EndMarker`      | draw.io arrow    | fill |
/// |-------------------|------------------|------|
/// | `None`            | `none`           | –    |
/// | `HollowTriangle`  | `block`          | 0    |
/// | `OpenArrow`       | `open`           | 0    |
/// | `FilledDiamond`   | `diamond`        | 1    |
/// | `HollowDiamond`   | `diamond`        | 0    |
/// | `ErOne`           | `ERone`          | 0    |
/// | `ErMany`          | `ERmany`         | 0    |
/// | `ErZeroOrOne`     | `ERzeroToOne`    | 0    |
/// | `ErOneOrMany`     | `ERoneToMany`    | 0    |
/// | `ErZeroOrMany`    | `ERzeroToMany`   | 0    |
///
/// Any future `#[non_exhaustive]` variant falls back to `none`.
fn drawio_arrow(marker: EndMarker) -> (&'static str, u8) {
    match marker {
        EndMarker::None => ("none", 0),
        EndMarker::HollowTriangle => ("block", 0),
        EndMarker::OpenArrow => ("open", 0),
        EndMarker::FilledDiamond => ("diamond", 1),
        EndMarker::HollowDiamond => ("diamond", 0),
        EndMarker::ErOne => ("ERone", 0),
        EndMarker::ErMany => ("ERmany", 0),
        EndMarker::ErZeroOrOne => ("ERzeroToOne", 0),
        EndMarker::ErOneOrMany => ("ERoneToMany", 0),
        EndMarker::ErZeroOrMany => ("ERzeroToMany", 0),
        _ => ("none", 0),
    }
}

/// Build the HTML `value` for a [`CompartmentBox`] vertex: an optional
/// centered stereotype line, the centered bold title, then each compartment
/// as a left-aligned, `<hr>`-separated block of rows.
///
/// The returned string contains raw HTML tags; callers must [`xml_escape`] it
/// before embedding it in an XML `value="…"` attribute (draw.io stores HTML
/// labels entity-escaped and un-escapes them back to HTML at load time).
fn class_box_value(b: &CompartmentBox) -> String {
    let mut s = String::new();
    if let Some(st) = &b.stereotype {
        s.push_str(&format!(
            "<div style=\"text-align:center\">&#171;{}&#187;</div>",
            xml_escape(st)
        ));
    }
    s.push_str(&format!(
        "<div style=\"text-align:center\"><b>{}</b></div>",
        xml_escape(&b.title)
    ));
    for c in &b.compartments {
        s.push_str("<hr size=\"1\"/>");
        let rows: Vec<String> = c.rows.iter().map(|r| xml_escape(r)).collect();
        s.push_str(&rows.join("<br/>"));
    }
    s
}

fn render_class(layout: &ClassLayout) -> Result<String, RenderError> {
    let mut out = mxfile_header();

    for (i, b) in layout.boxes.iter().enumerate() {
        let r = &b.rect;
        out.push_str(&format!(
            "        <mxCell id=\"n{i}\" value=\"{}\" \
             style=\"rounded=1;whiteSpace=wrap;html=1;align=left;verticalAlign=top;\
             spacingLeft=6;spacingTop=4;spacingBottom=4;\" vertex=\"1\" parent=\"1\">\n\
             \x20         <mxGeometry x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" as=\"geometry\"/>\n\
             \x20       </mxCell>\n",
            xml_escape(&class_box_value(b)),
            f(r.x + MARGIN),
            f(r.y + MARGIN),
            f(r.width),
            f(r.height),
        ));
    }

    let find_box_idx =
        |id: &str| -> Option<usize> { layout.boxes.iter().position(|b| b.id.as_str() == id) };

    for (i, rel) in layout.relations.iter().enumerate() {
        let src_idx = find_box_idx(rel.from.as_str()).ok_or_else(|| RenderError::DanglingEdge {
            node_id: rel.from.to_string(),
        })?;
        let tgt_idx = find_box_idx(rel.to.as_str()).ok_or_else(|| RenderError::DanglingEdge {
            node_id: rel.to.to_string(),
        })?;

        let (start_arrow, start_fill) = drawio_arrow(rel.from_marker);
        let (end_arrow, end_fill) = drawio_arrow(rel.to_marker);
        let dashed = if rel.line == LineStyle::Dashed {
            "dashed=1;"
        } else {
            ""
        };
        let style = format!(
            "edgeStyle=none;html=1;{dashed}\
             startArrow={start_arrow};startFill={start_fill};\
             endArrow={end_arrow};endFill={end_fill};",
        );

        let label_value = xml_escape(rel.label.as_deref().unwrap_or(""));
        let wp = waypoints_xml_tuples(&rel.points);
        let has_children = !wp.is_empty();
        let edge_id = format!("e{i}");

        if has_children {
            out.push_str(&format!(
                "        <mxCell id=\"{edge_id}\" value=\"{label_value}\" style=\"{style}\" \
                 edge=\"1\" source=\"n{src_idx}\" target=\"n{tgt_idx}\" parent=\"1\">\n\
                 \x20         <mxGeometry relative=\"1\" as=\"geometry\">{wp}\n\
                 \x20         </mxGeometry>\n\
                 \x20       </mxCell>\n",
            ));
        } else {
            out.push_str(&format!(
                "        <mxCell id=\"{edge_id}\" value=\"{label_value}\" style=\"{style}\" \
                 edge=\"1\" source=\"n{src_idx}\" target=\"n{tgt_idx}\" parent=\"1\">\n\
                 \x20         <mxGeometry relative=\"1\" as=\"geometry\"/>\n\
                 \x20       </mxCell>\n",
            ));
        }

        // Multiplicity labels, positioned near each endpoint via a relative
        // edge-label child cell (x=-1 near the source, x=1 near the target;
        // draw.io interprets `x` on an edge label as a position along the
        // edge path in [-1, 1]).
        if let Some(m) = &rel.from_mult {
            out.push_str(&mult_label_cell(&edge_id, "from", m, -1.0));
        }
        if let Some(m) = &rel.to_mult {
            out.push_str(&mult_label_cell(&edge_id, "to", m, 1.0));
        }
    }

    out.push_str(mxfile_footer());
    Ok(out)
}

fn render_er(layout: &ClassLayout) -> Result<String, RenderError> {
    // ER layouts are structurally identical ClassLayouts (see
    // `kozue_layout::semantic::ErLayout`); reuse the same renderer.
    render_class(layout)
}

/// A small `edgeLabel` child cell showing a relation multiplicity near one
/// endpoint of edge `edge_id`.
fn mult_label_cell(edge_id: &str, side: &str, value: &str, x: f64) -> String {
    format!(
        "        <mxCell id=\"{edge_id}-mult-{side}\" value=\"{}\" \
         style=\"edgeLabel;html=1;align=center;verticalAlign=middle;\" \
         vertex=\"1\" connectable=\"0\" parent=\"{edge_id}\">\n\
         \x20         <mxGeometry x=\"{}\" relative=\"1\" as=\"geometry\">\n\
         \x20           <mxPoint as=\"offset\"/>\n\
         \x20         </mxGeometry>\n\
         \x20       </mxCell>\n",
        xml_escape(value),
        f(x),
    )
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

    // Helper: class diagram layout with an inheritance relation (hollow
    // triangle) and a composition relation (filled diamond + multiplicities).
    fn class_layout() -> SemanticLayout {
        use kozue_ir::{ClassDiagram, ClassNode, ClassRelation, Diagram, Direction, EndMarker};
        let mut cd = ClassDiagram::new(Direction::Down);

        let mut animal = ClassNode::new("Animal", "Animal");
        animal.stereotype = Some("abstract".to_string());
        animal.attributes.push("+name: String".to_string());
        animal.methods.push("+speak(): void".to_string());
        cd.classes.insert("Animal".into(), animal);

        let mut dog = ClassNode::new("Dog", "Dog");
        dog.methods.push("+bark(): void".to_string());
        cd.classes.insert("Dog".into(), dog);

        let mut kennel = ClassNode::new("Kennel", "Kennel");
        kennel.attributes.push("+capacity: Int".to_string());
        cd.classes.insert("Kennel".into(), kennel);

        cd.relations.push(ClassRelation::new(
            "Dog",
            "Animal",
            EndMarker::None,
            EndMarker::HollowTriangle,
            kozue_ir::LineStyle::Solid,
            None,
            None,
            None,
        ));
        cd.relations.push(ClassRelation::new(
            "Kennel",
            "Dog",
            EndMarker::FilledDiamond,
            EndMarker::None,
            kozue_ir::LineStyle::Dashed,
            Some("houses".to_string()),
            Some("1".to_string()),
            Some("*".to_string()),
        ));

        let out = kozue_layout::layout_full(&Diagram::Class(cd)).expect("layout");
        out.semantic
    }

    // Helper: ER diagram layout with a crow's-foot relation.
    fn er_layout() -> SemanticLayout {
        use kozue_ir::{Diagram, EndMarker, ErAttribute, ErDiagram, ErEntity, ErRelation};
        let mut ed = ErDiagram::new();

        let mut customer = ErEntity::new("Customer", "CUSTOMER");
        customer
            .attributes
            .push(ErAttribute::new("int", "id", vec!["PK".to_string()], None));
        ed.entities.insert("Customer".into(), customer);

        let mut order = ErEntity::new("Order", "ORDER");
        order.attributes.push(ErAttribute::new(
            "int",
            "customer_id",
            vec!["FK".to_string()],
            None,
        ));
        ed.entities.insert("Order".into(), order);

        ed.relations.push(ErRelation::new(
            "Customer",
            "Order",
            EndMarker::ErOne,
            EndMarker::ErZeroOrMany,
            Some("places".to_string()),
            kozue_ir::LineStyle::Solid,
        ));

        let out = kozue_layout::layout_full(&Diagram::Er(ed)).expect("layout");
        out.semantic
    }

    // --- class rendering ---

    #[test]
    fn class_render_produces_box_with_compartments() {
        let layout = class_layout();
        let xml = render(&layout).expect("class render");
        assert!(xml.starts_with("<mxfile>"));
        assert!(
            xml.contains("rounded=1"),
            "class boxes are rounded rects: {xml}"
        );
        assert!(xml.contains("Animal"), "class name must appear: {xml}");
        assert!(
            xml.contains("+speak(): void"),
            "method compartment row must appear: {xml}"
        );
        assert!(
            xml.contains("&lt;hr"),
            "compartments are hr-separated (entity-escaped HTML): {xml}"
        );
    }

    #[test]
    fn class_render_maps_end_markers() {
        let layout = class_layout();
        let xml = render(&layout).expect("class render");
        assert!(
            xml.contains("endArrow=block;endFill=0"),
            "inheritance is a hollow block arrow: {xml}"
        );
        assert!(
            xml.contains("startArrow=diamond;startFill=1"),
            "composition is a filled diamond: {xml}"
        );
        assert!(xml.contains("dashed=1;"), "dashed relation: {xml}");
    }

    #[test]
    fn class_render_multiplicities_are_child_cells() {
        let layout = class_layout();
        let xml = render(&layout).expect("class render");
        assert!(xml.contains("value=\"1\""), "from_mult label: {xml}");
        assert!(xml.contains("value=\"*\""), "to_mult label: {xml}");
        assert!(
            xml.contains("edgeLabel"),
            "mult labels are edgeLabel cells: {xml}"
        );
    }

    #[test]
    fn class_render_is_deterministic() {
        let layout = class_layout();
        let xml1 = render(&layout).expect("render 1");
        let xml2 = render(&layout).expect("render 2");
        assert_eq!(xml1, xml2, "class render must be deterministic");
    }

    // --- er rendering ---

    #[test]
    fn er_render_produces_box_and_crowsfoot_markers() {
        let layout = er_layout();
        let xml = render(&layout).expect("er render");
        assert!(xml.contains("CUSTOMER"), "entity name must appear: {xml}");
        assert!(
            xml.contains("customer_id"),
            "attribute row must appear: {xml}"
        );
        assert!(
            xml.contains("startArrow=ERone"),
            "ErOne maps to ERone: {xml}"
        );
        assert!(
            xml.contains("endArrow=ERzeroToMany"),
            "ErZeroOrMany maps to ERzeroToMany: {xml}"
        );
        assert!(xml.contains("value=\"places\""), "relation label: {xml}");
    }

    #[test]
    fn er_render_is_deterministic() {
        let layout = er_layout();
        let xml1 = render(&layout).expect("render 1");
        let xml2 = render(&layout).expect("render 2");
        assert_eq!(xml1, xml2, "er render must be deterministic");
    }

    #[test]
    fn class_render_dangling_relation_is_error() {
        let layout = class_layout();
        let SemanticLayout::Class(mut cl) = layout else {
            panic!("expected class layout");
        };
        cl.relations[0].to = "does-not-exist".into();
        let err = render(&SemanticLayout::Class(cl)).unwrap_err();
        assert_eq!(
            err,
            RenderError::DanglingEdge {
                node_id: "does-not-exist".to_string()
            }
        );
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

    // Helper: build a graph layout with a labeled container via the real
    // layout pipeline.
    fn graph_with_container_layout() -> SemanticLayout {
        use kozue_ir::{ArrowType, Container, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "Alpha"));
        g.nodes.insert("b".into(), Node::new("b", "Beta"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::Triangle));
        let mut container = Container::new("x", Some("Group".to_string()));
        container.members.push("a".into());
        g.containers.push(container);
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        out.semantic
    }

    #[test]
    fn graph_with_no_containers_is_byte_identical() {
        let with_empty = graph_two_node_layout();
        let SemanticLayout::Graph(g) = &with_empty else {
            unreachable!()
        };
        assert!(g.containers.is_empty());
        let xml = render(&with_empty).expect("render");
        assert!(!xml.contains(" id=\"c0\""), "no container cell: {xml}");
    }

    #[test]
    fn graph_container_emits_dashed_backdrop_cell_before_nodes() {
        let layout = graph_with_container_layout();
        let xml = render(&layout).expect("render");
        assert!(
            xml.contains("<mxCell id=\"c0\" value=\"Group\" style=\"rounded=0;dashed=1;"),
            "{xml}"
        );
        let c0 = xml.find("id=\"c0\"").expect("container cell present");
        let n0 = xml.find("id=\"n0\"").expect("node cell present");
        assert!(c0 < n0, "container cell must be emitted before node cells");
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

    #[test]
    fn graph_edge_default_presentation_style_is_unchanged() {
        assert_eq!(
            graph_edge_style(
                ArrowType::Triangle,
                ArrowType::None,
                LineStyle::Solid,
                LineWeight::Normal,
                None,
                None,
            )
            .unwrap(),
            "edgeStyle=orthogonalEdgeStyle;"
        );
        assert_eq!(
            graph_edge_style(
                ArrowType::None,
                ArrowType::None,
                LineStyle::Solid,
                LineWeight::Normal,
                None,
                None,
            )
            .unwrap(),
            "edgeStyle=orthogonalEdgeStyle;endArrow=none;"
        );
    }

    #[test]
    fn graph_edge_from_arrow_maps_to_start_arrow_classic() {
        let style = graph_edge_style(
            ArrowType::Triangle,
            ArrowType::Triangle,
            LineStyle::Solid,
            LineWeight::Normal,
            None,
            None,
        )
        .unwrap();
        assert!(style.contains("startArrow=classic;"), "style: {style}");
    }

    #[test]
    fn graph_edge_line_maps_to_dashed_and_dotted() {
        let dashed = graph_edge_style(
            ArrowType::Triangle,
            ArrowType::None,
            LineStyle::Dashed,
            LineWeight::Normal,
            None,
            None,
        )
        .unwrap();
        assert!(dashed.contains("dashed=1;"), "dashed style: {dashed}");
        assert!(!dashed.contains("dashPattern"));

        let dotted = graph_edge_style(
            ArrowType::Triangle,
            ArrowType::None,
            LineStyle::Dotted,
            LineWeight::Normal,
            None,
            None,
        )
        .unwrap();
        assert!(
            dotted.contains("dashed=1;dashPattern=1 4;"),
            "dotted style: {dotted}"
        );
    }

    #[test]
    fn graph_edge_thick_weight_maps_to_stroke_width() {
        let style = graph_edge_style(
            ArrowType::Triangle,
            ArrowType::None,
            LineStyle::Solid,
            LineWeight::Thick,
            None,
            None,
        )
        .unwrap();
        assert!(style.contains("strokeWidth=3;"), "style: {style}");
    }

    #[test]
    fn graph_edge_ports_append_exit_entry_in_fixed_order() {
        let style = graph_edge_style(
            ArrowType::Triangle,
            ArrowType::None,
            LineStyle::Solid,
            LineWeight::Normal,
            Some(Port::East),
            Some(Port::West),
        )
        .unwrap();
        assert_eq!(
            style,
            "edgeStyle=orthogonalEdgeStyle;\
             exitX=1;exitY=0.5;exitDx=0;exitDy=0;\
             entryX=0;entryY=0.5;entryDx=0;entryDy=0;"
        );
    }

    #[test]
    fn graph_edge_ports_render_through_full_pipeline() {
        use kozue_ir::{
            ArrowType, Diagram, Direction, Edge, GraphDiagram, LineStyle, LineWeight, Node,
        };
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        g.edges.push(Edge::with_ports(
            "a",
            "b",
            None,
            ArrowType::Triangle,
            ArrowType::None,
            LineStyle::Solid,
            LineWeight::Normal,
            Some(Port::East),
            Some(Port::West),
        ));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let xml = render(&out.semantic).expect("render");
        assert!(xml.contains("exitX=1;exitY=0.5;"), "{xml}");
        assert!(xml.contains("entryX=0;entryY=0.5;"), "{xml}");
    }

    #[test]
    fn graph_render_new_edge_attrs_produce_byte_identical_default_case() {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::Triangle));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let xml = render(&out.semantic).expect("render");
        assert!(xml.contains("style=\"edgeStyle=orthogonalEdgeStyle;\""));
        assert!(!xml.contains("startArrow"));
        assert!(!xml.contains("dashed"));
        assert!(!xml.contains("strokeWidth"));
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
            xml.contains("id=\"final_inner\""),
            "final has an inner filled dot cell"
        );
        assert!(
            xml.contains("id=\"final_inner\" value=\"\" \
             style=\"ellipse;fillColor=#000000;strokeColor=#000000;\" vertex=\"1\" parent=\"final\""),
            "inner dot is a child of the outer ring so they move together"
        );
        assert!(
            !xml.contains("target=\"final_inner\""),
            "transitions target the outer ring, never the decorative inner dot"
        );
        assert!(
            xml.contains("ellipse;fillColor=none;strokeColor=#000000;"),
            "final outer ring is an unfilled ellipse"
        );
        assert!(
            !xml.contains("shape=doubleEllipse"),
            "final must not be a single solid doubleEllipse blob"
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

    // --- sequence rendering ---

    #[test]
    fn sequence_render_produces_umllifeline_vertices() {
        let layout = seq_layout();
        let xml = render(&layout).expect("sequence render");
        assert!(xml.starts_with("<mxfile>"));
        assert!(xml.contains("id=\"n0\""), "first participant is n0");
        assert!(xml.contains("id=\"n1\""), "second participant is n1");
        assert!(
            xml.contains("shape=umlLifeline"),
            "participants are umlLifeline vertices: {xml}"
        );
        // Values are display labels, not ids.
        assert!(xml.contains("value=\"Alice\""), "participant label Alice");
        assert!(xml.contains("value=\"Bob\""), "participant label Bob");
        assert!(
            !xml.contains("value=\"a\""),
            "raw participant id must not leak as a value"
        );
    }

    #[test]
    fn sequence_render_message_is_pinned_and_connected() {
        let layout = seq_layout();
        let xml = render(&layout).expect("sequence render");
        assert!(xml.contains("id=\"e0\""), "message is e0");
        assert!(
            xml.contains("source=\"n0\" target=\"n1\""),
            "message connects source lifeline to target lifeline: {xml}"
        );
        assert!(
            xml.contains("exitY=") && xml.contains("entryY="),
            "message endpoints are pinned via exitY/entryY: {xml}"
        );
        assert!(
            xml.contains("exitPerimeter=0") && xml.contains("entryPerimeter=0"),
            "fixed connection points bypass perimeter routing: {xml}"
        );
        assert!(xml.contains("value=\"hi\""), "message label is emitted");
    }

    #[test]
    fn sequence_render_exit_fraction_matches_message_y() {
        // The exitY fraction actually *emitted in the XML* must reproduce the
        // semantic message y: frac * vertex_height + vertex_y ≈ route[0].y.
        // Parses the rendered style so a formatter or denominator regression
        // in render_sequence() fails here, not only in the goldens.
        let layout = seq_layout();
        let SemanticLayout::Sequence(s) = &layout else {
            panic!("expected sequence layout");
        };
        let xml = render(&layout).expect("sequence render");
        let after = xml.split("exitY=").nth(1).expect("exitY in rendered style");
        let frac: f64 = after
            .split(';')
            .next()
            .unwrap()
            .parse()
            .expect("exitY parses as f64");
        let m = &s.messages[0];
        let src = s.participants.iter().find(|p| p.id == m.from).unwrap();
        let height = src.lifeline_y1 - src.header_rect.y;
        let reconstructed = frac * height + src.header_rect.y;
        assert!(
            (reconstructed - m.route[0].y).abs() < 0.1,
            "rendered exitY must reconstruct message y within 0.1px: got {reconstructed}, want {}",
            m.route[0].y
        );
    }

    #[test]
    fn sequence_render_is_deterministic() {
        let layout = seq_layout();
        let xml1 = render(&layout).expect("render 1");
        let xml2 = render(&layout).expect("render 2");
        assert_eq!(xml1, xml2, "sequence render must be deterministic");
    }

    #[test]
    fn frac_formats_six_decimals() {
        assert_eq!(frac(0.5), "0.500000");
        assert_eq!(frac(80.0 / 248.0), "0.322581");
    }

    // --- error displays ---

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
