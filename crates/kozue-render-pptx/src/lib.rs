//! Deterministic PowerPoint (`.pptx`) exporter for the SemanticLayout.
//!
//! ## Rationale
//!
//! Like [`kozue_render_drawio`](../kozue_render_drawio) and
//! `kozue-render-excalidraw`, this exporter reads the **semantic** layout
//! produced by [`kozue_layout::layout_full`] so each PowerPoint shape maps to
//! a meaningful diagram element (node, edge, pseudostate, participant,
//! message, ...) rather than to a raw drawing primitive.
//!
//! A `.pptx` file is a ZIP (OPC) container of OOXML parts. Unlike the other
//! exporters this one returns **bytes**, not a `String`.
//!
//! ## Determinism
//!
//! Output is byte-identical for the same input:
//! - The ZIP container is written by the hand-rolled [`zip::ZipWriter`]
//!   (STORE only, no external `zip`/`flate2` crate — see `zip.rs`).
//! - Every ZIP entry has a fixed DOS timestamp (1980-01-01 00:00:00), never
//!   the wall-clock time.
//! - `docProps/core.xml`'s `dcterms:created` / `dcterms:modified` are the
//!   fixed constant `"2024-01-01T00:00:00Z"`.
//! - ZIP entries are added in a fixed, hard-coded order (see [`build_pptx`]).
//! - Shape IDs are deterministic: `2, 3, 4, ...` in layout-declaration order
//!   (id `1` is reserved for the slide's group shape per the OOXML spec).
//! - No `HashMap` anywhere; all collections are `Vec` (iteration order =
//!   layout order).
//! - EMU coordinates are rounded to the nearest integer (`.round() as i64`).
//!
//! ## Coordinate space
//!
//! [`SemanticLayout`] coordinates are in CSS pixels (96 DPI), origin at
//! `(0, 0)`, y-axis pointing down — the same space the SVG/PNG/draw.io
//! renderers use. OOXML measures in EMU (914400 per inch, so 9525 per pixel
//! at 96 DPI). A fixed 20px margin (matching the other renderers) is added to
//! every position before the EMU conversion.
//!
//! ## Supported diagram types
//!
//! - [`SemanticLayout::Graph`] — nodes become rounded-rectangle shapes
//!   (`p:sp`); edges become connectors (`p:cxnSp`) tracing the full route.
//! - [`SemanticLayout::State`] — named states become rounded rectangles; the
//!   initial pseudostate becomes a filled ellipse; the final pseudostate
//!   becomes an outer stroked ring plus an inner filled dot (two `p:sp`
//!   ellipses); transitions become connectors.
//! - [`SemanticLayout::Sequence`] — participants become a header rectangle
//!   plus a dashed vertical lifeline connector; messages become connectors
//!   (dashed for `LineStyle::Dashed`).
//! - [`SemanticLayout::Class`] / [`SemanticLayout::Er`] — each
//!   [`CompartmentBox`] becomes a rectangular shape whose text body has one
//!   paragraph per title/stereotype/attribute/method row, plus a thin
//!   connector line at each compartment boundary. Each [`RelationLayout`]
//!   becomes a connector whose `headEnd`/`tailEnd` approximate the UML/ER end
//!   markers (OOXML's `ST_LineEndType` has only five shapes, far fewer than
//!   kozue's ten [`EndMarker`] variants — see [`pptx_line_end`] for the
//!   documented, lossy mapping).
//!
//! ## Connector routing and labels
//!
//! Connectors trace their layout `route` faithfully (see [`connector_shape`]):
//! a two-point (or otherwise collinear) route renders as a `straightConnector1`
//! connector; a route with genuine bend points renders as a freeform `p:sp`
//! `custGeom` polyline that visits every point, so multi-layer graph edges,
//! state self-loops, and sequence self-messages keep their real bent shape
//! instead of collapsing to a straight segment. (Both are representations real
//! PowerPoint accepts: `custGeom` on a `p:cxnSp` connector is rejected, so a
//! polyline must be a freeform shape.)
//!
//! Edge / transition / message labels are rendered as small white-filled,
//! borderless text boxes centered on the layout `label_anchor` (see
//! [`label_box_shape`]) — the white fill masks the connector behind the glyphs,
//! matching the draw.io edge-label look. Node, state, and participant labels
//! are drawn directly in their `p:sp` shapes.

mod templates;
mod zip;

use kozue_ir::{ArrowType, EndMarker, LineStyle};
use kozue_layout::semantic::{
    ClassLayout, CompartmentBox, GraphLayout, Point, SemanticLayout, SequenceLayout,
    StateEndpointId, StateLayout,
};

/// Fixed scene margin (px), matching the SVG/PNG/draw.io/Excalidraw renderers.
const MARGIN: f64 = 20.0;
/// EMU per CSS pixel at 96 DPI (914400 EMU per inch / 96 px per inch).
const EMU_PER_PX: f64 = 9525.0;
/// Nominal font size (px) used only to size a connector's label box. The label
/// run is emitted at the matching 12 pt (`sz="1200"`); the box is sized a touch
/// generously so the white fill always bounds the glyphs (no line bleed).
const LABEL_FONT_PX: f64 = 16.0;

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

/// An error that prevents a [`SemanticLayout`] from being exported to
/// PowerPoint.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    /// The diagram type is not supported by this exporter.
    UnsupportedDiagram {
        /// Human-readable description of the unsupported variant.
        kind: &'static str,
    },
    /// A graph edge or sequence message references a node/participant ID
    /// that is not present in the layout. Silently dropping it would produce
    /// misleading output.
    DanglingEdge {
        /// The missing node/participant ID.
        node_id: String,
    },
    /// A state transition references an endpoint that cannot be resolved.
    /// Covers unknown `StateEndpointId` variants added in the future (the
    /// type is `#[non_exhaustive]`).
    UnknownEndpoint {
        /// Human-readable description of the unresolved endpoint.
        description: String,
    },
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::UnsupportedDiagram { kind } => {
                write!(f, "PowerPoint export does not support {kind} diagrams")
            }
            RenderError::DanglingEdge { node_id } => {
                write!(
                    f,
                    "PowerPoint export: edge references unknown node \"{node_id}\""
                )
            }
            RenderError::UnknownEndpoint { description } => {
                write!(
                    f,
                    "PowerPoint export: cannot resolve transition endpoint: {description}"
                )
            }
        }
    }
}

impl std::error::Error for RenderError {}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Export a [`SemanticLayout`] to a `.pptx` (OPC ZIP) byte stream.
///
/// Returns byte-identical output for the same input on any target (see the
/// module docs for the determinism guarantees).
///
/// # Errors
///
/// Returns [`RenderError::UnsupportedDiagram`] for any future layout variants
/// with no slide representation yet. Returns [`RenderError::DanglingEdge`] if
/// a graph edge or sequence message references an unknown node/participant
/// ID. Returns [`RenderError::UnknownEndpoint`] if a state transition
/// endpoint cannot be resolved.
pub fn render(layout: &SemanticLayout) -> Result<Vec<u8>, RenderError> {
    let slide_xml = match layout {
        SemanticLayout::Graph(g) => render_graph(g)?,
        SemanticLayout::State(s) => render_state(s)?,
        SemanticLayout::Sequence(seq) => render_sequence(seq)?,
        SemanticLayout::Class(c) => render_class(c)?,
        SemanticLayout::Er(e) => render_er(e)?,
        _ => return Err(RenderError::UnsupportedDiagram { kind: "unknown" }),
    };
    Ok(build_pptx(&slide_xml))
}

// ---------------------------------------------------------------------------
// EMU coordinate conversion
// ---------------------------------------------------------------------------

/// Convert a scene position (px) to EMU, applying the fixed scene margin.
/// Deterministic: rounds to the nearest integer EMU.
fn emu_pos(px: f64) -> i64 {
    ((px + MARGIN) * EMU_PER_PX).round() as i64
}

/// Convert a scene length (px, no margin) to EMU.
fn emu_len(px: f64) -> i64 {
    (px * EMU_PER_PX).round() as i64
}

// ---------------------------------------------------------------------------
// XML escaping
// ---------------------------------------------------------------------------

/// Escape `< > & " '` for use in XML attribute values and text content.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Shape ID allocator
// ---------------------------------------------------------------------------

/// Deterministic shape-ID allocator: starts at 2 (id `1` is reserved for the
/// slide's group shape) and increments by 1 per shape, in emission order.
struct IdAlloc {
    next: u32,
}

impl IdAlloc {
    fn new() -> Self {
        IdAlloc { next: 2 }
    }

    fn next(&mut self) -> u32 {
        let id = self.next;
        self.next += 1;
        id
    }
}

// ---------------------------------------------------------------------------
// Shape XML builders
// ---------------------------------------------------------------------------

/// A rounded-rectangle shape with centered text (used for graph nodes, state
/// boxes, and sequence participant headers).
fn rect_shape(id: u32, name: &str, x: i64, y: i64, w: i64, h: i64, label: &str) -> String {
    // Escape `name` for the same reason `label` is escaped: it can carry a
    // user-supplied node/state/participant id. Current frontends reject XML
    // metacharacters in ids, but escaping keeps the attribute XML-safe by
    // construction rather than relying on that invariant.
    let name = xml_escape(name);
    let run = if label.is_empty() {
        String::new()
    } else {
        format!("<a:r><a:t>{}</a:t></a:r>", xml_escape(label))
    };
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/></a:xfrm>\
         <a:prstGeom prst=\"roundRect\"><a:avLst/></a:prstGeom>\
         <a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill>\
         <a:ln><a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill></a:ln></p:spPr>\
         <p:txBody><a:bodyPr anchor=\"ctr\"/><a:lstStyle/><a:p><a:pPr algn=\"ctr\"/>{run}</a:p></p:txBody></p:sp>",
    )
}

/// An ellipse shape (used for state pseudostates). `filled` selects a solid
/// black fill (initial pseudostate / final inner dot) vs. an unfilled ring
/// (final outer ring).
fn ellipse_shape(id: u32, name: &str, x: i64, y: i64, w: i64, h: i64, filled: bool) -> String {
    let name = xml_escape(name);
    let fill = if filled {
        "<a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill>"
    } else {
        "<a:noFill/>"
    };
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/></a:xfrm>\
         <a:prstGeom prst=\"ellipse\"><a:avLst/></a:prstGeom>\
         {fill}\
         <a:ln><a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill></a:ln></p:spPr>\
         <p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>",
    )
}

/// A connector tracing the full `route`.
///
/// The two branches use the two DrawingML representations that real PowerPoint
/// accepts (LibreOffice is more lenient, but PowerPoint rejects the whole file
/// on unexpected content):
///
/// - **Straight** (2-point or collinear route): a `p:cxnSp` connector with the
///   `straightConnector1` *connector* preset. (A plain `line` shape preset is
///   not a connector preset and can trip PowerPoint's repair on a `cxnSp`.)
/// - **Bent** (interior points, non-degenerate box): a `p:sp` *freeform shape*
///   with `custGeom`. PowerPoint only accepts preset connector geometries on a
///   `p:cxnSp`, so an arbitrary polyline must be a freeform `p:sp`, not a
///   `cxnSp` — that mismatch was the cause of "PowerPoint can't open" files.
///
/// `dashed` selects `prstDash="dash"` (sequence dashed messages / lifelines);
/// `arrow` selects a triangle tail arrowhead when not [`ArrowType::None`].
fn connector_shape(id: u32, name: &str, route: &[Point], dashed: bool, arrow: bool) -> String {
    connector_shape_ends(
        id,
        name,
        route,
        dashed,
        "none",
        if arrow { "triangle" } else { "none" },
    )
}

/// Like [`connector_shape`], but with independently configurable head
/// (route-first-point) and tail (route-last-point) arrowhead types — used by
/// class/ER relations, whose end markers can differ at each end. `head`/
/// `tail` are `ST_LineEndType` values (`"none"`, `"triangle"`, `"stealth"`,
/// `"diamond"`, `"oval"`, `"arrow"`).
fn connector_shape_ends(
    id: u32,
    name: &str,
    route: &[Point],
    dashed: bool,
    head: &str,
    tail: &str,
) -> String {
    let name = xml_escape(name);
    // Callers validate the route is non-empty before this point; guard anyway
    // so a stray empty route degrades to an (invisible) zero-size connector
    // rather than panicking.
    if route.is_empty() {
        return String::new();
    }
    let first = &route[0];
    let last = &route[route.len() - 1];

    // Bounding box over the whole route.
    let (mut min_x, mut min_y) = (first.x, first.y);
    let (mut max_x, mut max_y) = (first.x, first.y);
    for p in route {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    let w_px = max_x - min_x;
    let h_px = max_y - min_y;

    let dash_xml = if dashed {
        "<a:prstDash val=\"dash\"/>"
    } else {
        ""
    };
    // `headEnd` decorates the route's first point, `tailEnd` its last point
    // (matches this module's existing single-ended `arrow` convention, where
    // the arrowhead lands on the route's last/"tail" point).
    let head_xml = if head == "none" {
        String::new()
    } else {
        format!("<a:headEnd type=\"{head}\"/>")
    };
    let tail_xml = if tail == "none" {
        String::new()
    } else {
        format!("<a:tailEnd type=\"{tail}\"/>")
    };
    let ln = format!(
        "<a:ln><a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill>{dash_xml}{head_xml}{tail_xml}</a:ln>"
    );

    // A freeform polyline is only meaningful when there are interior bend
    // points AND the bounding box has area on both axes. A route that is
    // collinear on one axis (w or h == 0) is exactly a straight segment, and a
    // `custGeom` path space with a zero dimension is degenerate — so those fall
    // through to the preset `line` below.
    let use_polyline = route.len() > 2 && w_px > 0.0 && h_px > 0.0;

    if use_polyline {
        let off_x = emu_pos(min_x);
        let off_y = emu_pos(min_y);
        let cx = emu_len(w_px);
        let cy = emu_len(h_px);
        // Path coordinate space == shape extent, so every path point maps 1:1
        // onto the shape's local coordinates.
        let mut path = String::new();
        for (idx, p) in route.iter().enumerate() {
            let px = emu_len(p.x - min_x);
            let py = emu_len(p.y - min_y);
            let tag = if idx == 0 { "moveTo" } else { "lnTo" };
            path.push_str(&format!("<a:{tag}><a:pt x=\"{px}\" y=\"{py}\"/></a:{tag}>"));
        }
        // Freeform shape (`p:sp`), NOT a connector: PowerPoint only accepts
        // preset connector geometries on a `p:cxnSp`.
        format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
             <p:spPr><a:xfrm><a:off x=\"{off_x}\" y=\"{off_y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
             <a:pathLst><a:path w=\"{cx}\" h=\"{cy}\" fill=\"none\">{path}</a:path></a:pathLst></a:custGeom>\
             <a:noFill/>{ln}</p:spPr>\
             <p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>",
        )
    } else {
        // Straight `straightConnector1` from `first` to `last`, positioned via
        // the bounding box plus flip flags (the connector runs top-left →
        // bottom-right in its own box; flips select the actual direction so the
        // tail arrowhead lands on `last`).
        let dx = last.x - first.x;
        let dy = last.y - first.y;
        let mut flip_attrs = String::new();
        if dx < 0.0 {
            flip_attrs.push_str(" flipH=\"1\"");
        }
        if dy < 0.0 {
            flip_attrs.push_str(" flipV=\"1\"");
        }
        let x = emu_pos(min_x);
        let y = emu_pos(min_y);
        let cx = emu_len(dx.abs());
        let cy = emu_len(dy.abs());
        format!(
            "<p:cxnSp><p:nvCxnSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr>\
             <p:spPr><a:xfrm{flip_attrs}><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"straightConnector1\"><a:avLst/></a:prstGeom>\
             {ln}</p:spPr></p:cxnSp>",
        )
    }
}

/// A borderless, white-filled text box centered on `anchor`, used to place a
/// connector's label (edge / transition / message) on top of the line so it
/// stays readable. Text is a single non-wrapping 12 pt line; the white fill
/// masks the connector behind the glyphs.
fn label_box_shape(id: u32, name: &str, anchor: &Point, label: &str) -> String {
    let name = xml_escape(name);
    let chars = label.chars().count().max(1) as f64;
    let w_px = chars * LABEL_FONT_PX * 0.7;
    let h_px = LABEL_FONT_PX * 1.4;
    let x = emu_pos(anchor.x - w_px / 2.0);
    let y = emu_pos(anchor.y - h_px / 2.0);
    let w = emu_len(w_px);
    let h = emu_len(h_px);
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>\
         <a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill><a:ln><a:noFill/></a:ln></p:spPr>\
         <p:txBody><a:bodyPr wrap=\"none\" lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\" anchor=\"ctr\"/><a:lstStyle/>\
         <a:p><a:pPr algn=\"ctr\"/><a:r><a:rPr sz=\"1200\"/><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>",
        text = xml_escape(label),
    )
}

// ---------------------------------------------------------------------------
// Slide skeleton
// ---------------------------------------------------------------------------

fn slide_xml(shapes: &str) -> String {
    format!(
        "{decl}<p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
         <p:cSld><p:spTree>\
         <p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
         <p:grpSpPr/>\
         {shapes}\
         </p:spTree></p:cSld>\
         <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>\
         </p:sld>",
        decl = templates::XML_DECL,
    )
}

// ---------------------------------------------------------------------------
// Graph diagram renderer
// ---------------------------------------------------------------------------

fn render_graph(g: &GraphLayout) -> Result<String, RenderError> {
    let mut ids = IdAlloc::new();
    let mut shapes = String::new();

    for (i, node) in g.nodes.iter().enumerate() {
        let r = &node.rect;
        shapes.push_str(&rect_shape(
            ids.next(),
            &format!("Node {i}"),
            emu_pos(r.x),
            emu_pos(r.y),
            emu_len(r.width),
            emu_len(r.height),
            &node.label,
        ));
    }

    let find_node = |id: &str| -> Option<&kozue_layout::semantic::NodeLayout> {
        g.nodes.iter().find(|n| n.id == id)
    };

    for (i, edge) in g.edges.iter().enumerate() {
        find_node(&edge.from.id).ok_or_else(|| RenderError::DanglingEdge {
            node_id: edge.from.id.clone(),
        })?;
        find_node(&edge.to.id).ok_or_else(|| RenderError::DanglingEdge {
            node_id: edge.to.id.clone(),
        })?;
        if edge.route.is_empty() {
            return Err(RenderError::DanglingEdge {
                node_id: edge.from.id.clone(),
            });
        }
        shapes.push_str(&connector_shape(
            ids.next(),
            &format!("Edge {i}"),
            &edge.route,
            false,
            edge.arrow != ArrowType::None,
        ));
        if let (Some(label), Some(anchor)) = (&edge.label, &edge.label_anchor) {
            shapes.push_str(&label_box_shape(
                ids.next(),
                &format!("Edge {i} label"),
                anchor,
                label,
            ));
        }
    }

    Ok(slide_xml(&shapes))
}

// ---------------------------------------------------------------------------
// State diagram renderer
// ---------------------------------------------------------------------------

fn render_state(s: &StateLayout) -> Result<String, RenderError> {
    let mut ids = IdAlloc::new();
    let mut shapes = String::new();

    for state in &s.states {
        let r = &state.rect;
        shapes.push_str(&rect_shape(
            ids.next(),
            &format!("State {}", state.id),
            emu_pos(r.x),
            emu_pos(r.y),
            emu_len(r.width),
            emu_len(r.height),
            &state.label,
        ));
    }

    if let Some(init) = &s.initial {
        let cx = init.center.x;
        let cy = init.center.y;
        let r = init.radius;
        shapes.push_str(&ellipse_shape(
            ids.next(),
            "Initial",
            emu_pos(cx - r),
            emu_pos(cy - r),
            emu_len(r * 2.0),
            emu_len(r * 2.0),
            true,
        ));
    }

    if let Some(fin) = &s.final_state {
        let cx = fin.center.x;
        let cy = fin.center.y;
        let ro = fin.outer_radius;
        let ri = fin.inner_radius;
        // Outer ring: unfilled.
        shapes.push_str(&ellipse_shape(
            ids.next(),
            "Final",
            emu_pos(cx - ro),
            emu_pos(cy - ro),
            emu_len(ro * 2.0),
            emu_len(ro * 2.0),
            false,
        ));
        // Inner dot: filled, drawn on top of the ring.
        shapes.push_str(&ellipse_shape(
            ids.next(),
            "Final inner",
            emu_pos(cx - ri),
            emu_pos(cy - ri),
            emu_len(ri * 2.0),
            emu_len(ri * 2.0),
            true,
        ));
    }

    // Resolve a StateEndpointId to its scene-space anchor point (center),
    // used only for validating the endpoint exists (the connector itself
    // uses the transition's own `route`, which already carries resolved
    // coordinates from the layout pass).
    let endpoint_exists = |ep: &StateEndpointId| -> Result<(), RenderError> {
        match ep {
            StateEndpointId::State(id) => {
                if s.states.iter().any(|st| st.id == *id) {
                    Ok(())
                } else {
                    Err(RenderError::UnknownEndpoint {
                        description: format!("state \"{id}\" not found in layout"),
                    })
                }
            }
            StateEndpointId::Initial => {
                if s.initial.is_some() {
                    Ok(())
                } else {
                    Err(RenderError::UnknownEndpoint {
                        description: "Initial pseudostate referenced but not present in layout"
                            .to_string(),
                    })
                }
            }
            StateEndpointId::Final => {
                if s.final_state.is_some() {
                    Ok(())
                } else {
                    Err(RenderError::UnknownEndpoint {
                        description: "Final pseudostate referenced but not present in layout"
                            .to_string(),
                    })
                }
            }
            _ => Err(RenderError::UnknownEndpoint {
                description: format!("unrecognised StateEndpointId variant: {ep:?}"),
            }),
        }
    };

    for (i, tr) in s.transitions.iter().enumerate() {
        endpoint_exists(&tr.from)?;
        endpoint_exists(&tr.to)?;
        if tr.route.is_empty() {
            return Err(RenderError::UnknownEndpoint {
                description: format!("transition {i} has an empty route"),
            });
        }
        shapes.push_str(&connector_shape(
            ids.next(),
            &format!("Transition {i}"),
            &tr.route,
            false,
            true, // state transitions are always directed
        ));
        if let (Some(label), Some(anchor)) = (&tr.label, &tr.label_anchor) {
            shapes.push_str(&label_box_shape(
                ids.next(),
                &format!("Transition {i} label"),
                anchor,
                label,
            ));
        }
    }

    Ok(slide_xml(&shapes))
}

// ---------------------------------------------------------------------------
// Sequence diagram renderer
// ---------------------------------------------------------------------------

fn render_sequence(s: &SequenceLayout) -> Result<String, RenderError> {
    let mut ids = IdAlloc::new();
    let mut shapes = String::new();

    for p in &s.participants {
        let r = &p.header_rect;
        shapes.push_str(&rect_shape(
            ids.next(),
            &format!("Participant {}", p.id),
            emu_pos(r.x),
            emu_pos(r.y),
            emu_len(r.width),
            emu_len(r.height),
            &p.label,
        ));
        // Lifeline: dashed vertical connector spanning the column.
        let lifeline = [
            Point::new(p.lifeline_x, p.lifeline_y0),
            Point::new(p.lifeline_x, p.lifeline_y1),
        ];
        shapes.push_str(&connector_shape(
            ids.next(),
            &format!("Lifeline {}", p.id),
            &lifeline,
            true,
            false,
        ));
    }

    let find_participant = |id: &str| -> bool { s.participants.iter().any(|p| p.id == id) };

    for (i, m) in s.messages.iter().enumerate() {
        if !find_participant(&m.from) {
            return Err(RenderError::DanglingEdge {
                node_id: m.from.clone(),
            });
        }
        if !find_participant(&m.to) {
            return Err(RenderError::DanglingEdge {
                node_id: m.to.clone(),
            });
        }
        if m.route.is_empty() {
            return Err(RenderError::DanglingEdge {
                node_id: m.from.clone(),
            });
        }
        shapes.push_str(&connector_shape(
            ids.next(),
            &format!("Message {i}"),
            &m.route,
            m.line == LineStyle::Dashed,
            m.arrow != ArrowType::None,
        ));
        if let (Some(label), Some(anchor)) = (&m.label, &m.label_anchor) {
            shapes.push_str(&label_box_shape(
                ids.next(),
                &format!("Message {i} label"),
                anchor,
                label,
            ));
        }
    }

    Ok(slide_xml(&shapes))
}

// ---------------------------------------------------------------------------
// Class / ER diagram renderer
// ---------------------------------------------------------------------------

/// Map an [`EndMarker`] to an OOXML `ST_LineEndType` value.
///
/// `ST_LineEndType` only has five shapes (`none` / `triangle` / `stealth` /
/// `diamond` / `oval` / `arrow`) — far fewer than kozue's ten [`EndMarker`]
/// variants — so this mapping is lossy by necessity: `HollowTriangle` and
/// `HollowDiamond` share their filled counterparts' shape (OOXML line ends
/// have no independent fill flag), and the ER crow's-foot markers collapse
/// onto `oval` (bar-like cardinalities) or `arrow` (crow-like cardinalities),
/// each losing the paired bar/circle it would carry in a full crow's-foot
/// rendering.
///
/// | `EndMarker`      | `ST_LineEndType` | notes                        |
/// |-------------------|-------------------|-------------------------------|
/// | `None`            | `none`            |                               |
/// | `HollowTriangle`  | `triangle`        | loses "hollow" (no fill flag) |
/// | `OpenArrow`       | `arrow`           |                               |
/// | `FilledDiamond`   | `diamond`         |                               |
/// | `HollowDiamond`   | `diamond`         | loses "hollow" (no fill flag) |
/// | `ErOne`           | `oval`            | approximates the bar          |
/// | `ErMany`          | `arrow`           | approximates the crow's foot  |
/// | `ErZeroOrOne`      | `oval`            | loses the paired bar          |
/// | `ErOneOrMany`      | `arrow`           | loses the paired bar          |
/// | `ErZeroOrMany`     | `arrow`           | loses the paired circle       |
fn pptx_line_end(marker: EndMarker) -> &'static str {
    match marker {
        EndMarker::None => "none",
        EndMarker::HollowTriangle => "triangle",
        EndMarker::OpenArrow => "arrow",
        EndMarker::FilledDiamond => "diamond",
        EndMarker::HollowDiamond => "diamond",
        EndMarker::ErOne => "oval",
        EndMarker::ErMany => "arrow",
        EndMarker::ErZeroOrOne => "oval",
        EndMarker::ErOneOrMany => "arrow",
        EndMarker::ErZeroOrMany => "arrow",
        _ => "none",
    }
}

/// A rectangular shape for a [`CompartmentBox`]: one centered paragraph for
/// the stereotype (if any) and title, followed by one left-aligned paragraph
/// per compartment row.
/// Horizontal text inset (px) inside a compartment text box, matching the
/// left padding the layout engine reserves for compartment rows.
const COMPARTMENT_PAD_X: f64 = 6.0;

/// A border-only rounded/rect box outline (white fill, black stroke, no text).
/// The title and compartment text are emitted as *separate* overlaid text
/// boxes at their own semantic y-ranges (see [`region_text_shape`]) so that
/// each block of text starts exactly at its compartment divider instead of
/// being reflowed from the box top by PowerPoint's own line layout.
fn box_outline_shape(id: u32, name: &str, x: i64, y: i64, w: i64, h: i64) -> String {
    let name = xml_escape(name);
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>\
         <a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill>\
         <a:ln><a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill></a:ln></p:spPr>\
         <p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>",
    )
}

/// A transparent, borderless text box occupying a fixed scene-space rectangle,
/// used to place one compartment's (or the title's) text exactly within its
/// semantic y-range. Paragraph alignment is baked into `paragraphs`; `anchor`
/// is the vertical anchor (`"ctr"` / `"t"`). Insets are zeroed so the text's
/// top edge coincides with the box's top edge (= the compartment divider).
/// Mirrors [`label_box_shape`]'s zero-inset construction.
#[allow(clippy::too_many_arguments)]
fn region_text_shape(
    id: u32,
    name: &str,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    paragraphs: &str,
    anchor: &str,
) -> String {
    let name = xml_escape(name);
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>\
         <a:noFill/><a:ln><a:noFill/></a:ln></p:spPr>\
         <p:txBody><a:bodyPr wrap=\"square\" lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\" anchor=\"{anchor}\"/>\
         <a:lstStyle/>{paragraphs}</p:txBody></p:sp>",
    )
}

/// Emit all shapes for one class/ER [`CompartmentBox`]: a border-only outline,
/// a centered title text box spanning `[rect.y, first_compartment_top_y]`, and
/// one left/top-aligned text box per compartment spanning
/// `[c.top_y, next_top_y]`. Each text box's top edge equals a divider y, so
/// the text stays aligned with the divider connectors drawn in `render_class`.
fn compartment_box_shapes(ids: &mut IdAlloc, b: &CompartmentBox) -> String {
    let r = &b.rect;
    let mut out = String::new();

    // Outer outline (border + white fill, no text).
    out.push_str(&box_outline_shape(
        ids.next(),
        &format!("Box {}", b.id),
        emu_pos(r.x),
        emu_pos(r.y),
        emu_len(r.width),
        emu_len(r.height),
    ));

    // Title region: from the box top down to the first compartment divider (or
    // the whole box height when there are no compartments).
    let title_bottom = b
        .compartments
        .first()
        .map(|c| c.top_y)
        .unwrap_or(r.y + r.height);
    let title_h = (title_bottom - r.y).max(1.0);
    let mut title_p = String::new();
    if let Some(st) = &b.stereotype {
        title_p.push_str(&format!(
            "<a:p><a:pPr algn=\"ctr\"/><a:r><a:rPr sz=\"1000\" i=\"1\"/><a:t>{}</a:t></a:r></a:p>",
            xml_escape(&format!("\u{ab}{st}\u{bb}"))
        ));
    }
    title_p.push_str(&format!(
        "<a:p><a:pPr algn=\"ctr\"/><a:r><a:rPr sz=\"1200\" b=\"1\"/><a:t>{}</a:t></a:r></a:p>",
        xml_escape(&b.title)
    ));
    out.push_str(&region_text_shape(
        ids.next(),
        &format!("Box {} title", b.id),
        emu_pos(r.x),
        emu_pos(r.y),
        emu_len(r.width),
        emu_len(title_h),
        &title_p,
        "ctr",
    ));

    // One text box per compartment, spanning [top_y, next top_y]. Left/top
    // aligned, with a small horizontal inset matching the layout's padding.
    for (ci, c) in b.compartments.iter().enumerate() {
        let bottom = b
            .compartments
            .get(ci + 1)
            .map(|c2| c2.top_y)
            .unwrap_or(r.y + r.height);
        let sect_h = (bottom - c.top_y).max(1.0);
        let mut sect_p = String::new();
        for row in &c.rows {
            sect_p.push_str(&format!(
                "<a:p><a:pPr algn=\"l\"/><a:r><a:rPr sz=\"1100\"/><a:t>{}</a:t></a:r></a:p>",
                xml_escape(row)
            ));
        }
        out.push_str(&region_text_shape(
            ids.next(),
            &format!("Box {} section {ci}", b.id),
            emu_pos(r.x + COMPARTMENT_PAD_X),
            emu_pos(c.top_y),
            emu_len((r.width - 2.0 * COMPARTMENT_PAD_X).max(1.0)),
            emu_len(sect_h),
            &sect_p,
            "t",
        ));
    }

    out
}

fn render_class(layout: &ClassLayout) -> Result<String, RenderError> {
    let mut ids = IdAlloc::new();
    let mut shapes = String::new();

    for b in &layout.boxes {
        let r = &b.rect;
        shapes.push_str(&compartment_box_shapes(&mut ids, b));
        // A thin divider connector at each compartment's top boundary,
        // spanning the box width.
        for c in &b.compartments {
            let divider = [Point::new(r.x, c.top_y), Point::new(r.x + r.width, c.top_y)];
            shapes.push_str(&connector_shape_ends(
                ids.next(),
                &format!("Box {} divider", b.id),
                &divider,
                false,
                "none",
                "none",
            ));
        }
    }

    let find_box = |id: &str| -> bool { layout.boxes.iter().any(|b| b.id == id) };

    for (i, rel) in layout.relations.iter().enumerate() {
        if !find_box(&rel.from) {
            return Err(RenderError::DanglingEdge {
                node_id: rel.from.clone(),
            });
        }
        if !find_box(&rel.to) {
            return Err(RenderError::DanglingEdge {
                node_id: rel.to.clone(),
            });
        }
        let route: Vec<Point> = rel.points.iter().map(|&(x, y)| Point::new(x, y)).collect();
        if route.is_empty() {
            return Err(RenderError::DanglingEdge {
                node_id: rel.from.clone(),
            });
        }
        shapes.push_str(&connector_shape_ends(
            ids.next(),
            &format!("Relation {i}"),
            &route,
            rel.line == LineStyle::Dashed,
            pptx_line_end(rel.from_marker),
            pptx_line_end(rel.to_marker),
        ));
        if let Some(label) = &rel.label {
            let mid = &route[route.len() / 2];
            shapes.push_str(&label_box_shape(
                ids.next(),
                &format!("Relation {i} label"),
                mid,
                label,
            ));
        }
        if let Some(m) = &rel.from_mult {
            shapes.push_str(&label_box_shape(
                ids.next(),
                &format!("Relation {i} from multiplicity"),
                &route[0],
                m,
            ));
        }
        if let Some(m) = &rel.to_mult {
            shapes.push_str(&label_box_shape(
                ids.next(),
                &format!("Relation {i} to multiplicity"),
                &route[route.len() - 1],
                m,
            ));
        }
    }

    Ok(slide_xml(&shapes))
}

fn render_er(layout: &ClassLayout) -> Result<String, RenderError> {
    // ER layouts are structurally identical ClassLayouts (see
    // `kozue_layout::semantic::ErLayout`); reuse the same renderer.
    render_class(layout)
}

// ---------------------------------------------------------------------------
// Package assembly
// ---------------------------------------------------------------------------

/// Assemble the complete `.pptx` ZIP (OPC) package, adding parts in a fixed,
/// hard-coded order so the byte layout depends only on `slide_xml`.
fn build_pptx(slide_xml: &str) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new();
    zw.add("[Content_Types].xml", templates::CONTENT_TYPES.as_bytes());
    zw.add("_rels/.rels", templates::ROOT_RELS.as_bytes());
    zw.add("docProps/app.xml", templates::DOC_PROPS_APP.as_bytes());
    zw.add("docProps/core.xml", templates::DOC_PROPS_CORE.as_bytes());
    zw.add("ppt/presentation.xml", templates::PRESENTATION.as_bytes());
    zw.add(
        "ppt/_rels/presentation.xml.rels",
        templates::PRESENTATION_RELS.as_bytes(),
    );
    zw.add(
        "ppt/slideMasters/slideMaster1.xml",
        templates::SLIDE_MASTER.as_bytes(),
    );
    zw.add(
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        templates::SLIDE_MASTER_RELS.as_bytes(),
    );
    zw.add(
        "ppt/slideLayouts/slideLayout1.xml",
        templates::SLIDE_LAYOUT.as_bytes(),
    );
    zw.add(
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        templates::SLIDE_LAYOUT_RELS.as_bytes(),
    );
    zw.add("ppt/theme/theme1.xml", templates::THEME.as_bytes());
    zw.add("ppt/slides/slide1.xml", slide_xml.as_bytes());
    zw.add(
        "ppt/slides/_rels/slide1.xml.rels",
        templates::SLIDE_RELS.as_bytes(),
    );
    zw.finish()
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

    // Helper: build a basic state diagram layout.
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

    // Helper: class diagram layout with inheritance + composition relations.
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

        cd.relations.push(ClassRelation::new(
            "Dog",
            "Animal",
            EndMarker::None,
            EndMarker::HollowTriangle,
            LineStyle::Solid,
            None,
            None,
            None,
        ));
        cd.relations.push(ClassRelation::new(
            "Dog",
            "Animal",
            EndMarker::FilledDiamond,
            EndMarker::None,
            LineStyle::Dashed,
            Some("has".to_string()),
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
            LineStyle::Solid,
        ));

        let out = kozue_layout::layout_full(&Diagram::Er(ed)).expect("layout");
        out.semantic
    }

    // --- class / er rendering ---

    #[test]
    fn render_class_produces_valid_zip() {
        let layout = class_layout();
        let bytes = render(&layout).expect("class render");
        assert_valid_zip_shape(&bytes);
    }

    #[test]
    fn render_er_produces_valid_zip() {
        let layout = er_layout();
        let bytes = render(&layout).expect("er render");
        assert_valid_zip_shape(&bytes);
    }

    #[test]
    fn class_slide_contains_boxes_and_markers() {
        let layout = class_layout();
        let bytes = render(&layout).expect("render");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("<p:sp>"), "compartment boxes are p:sp shapes");
        assert!(text.contains("Animal"), "class name must appear");
        assert!(text.contains("+speak(): void"), "method row must appear");
        assert!(
            text.contains("<a:headEnd type=\"diamond\"/>")
                || text.contains("<a:tailEnd type=\"diamond\"/>"),
            "composition maps to a diamond line end: {text}"
        );
        assert!(
            text.contains("<a:headEnd type=\"triangle\"/>")
                || text.contains("<a:tailEnd type=\"triangle\"/>"),
            "inheritance maps to a triangle line end: {text}"
        );
        assert!(text.contains("has"), "relation label must appear");
        assert!(text.contains(">1<"), "from multiplicity must appear");
        assert!(text.contains(">*<"), "to multiplicity must appear");
    }

    #[test]
    fn er_slide_contains_entity_rows_and_crowsfoot_approximation() {
        let layout = er_layout();
        let bytes = render(&layout).expect("render");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("customer_id"), "attribute row must appear");
        assert!(
            text.contains("<a:headEnd type=\"oval\"/>")
                || text.contains("<a:tailEnd type=\"oval\"/>"),
            "ErOne maps to an oval line end: {text}"
        );
        assert!(
            text.contains("<a:headEnd type=\"arrow\"/>")
                || text.contains("<a:tailEnd type=\"arrow\"/>"),
            "ErZeroOrMany maps to an arrow line end: {text}"
        );
        assert!(text.contains("places"), "relation label must appear");
    }

    #[test]
    fn render_class_is_deterministic() {
        let layout = class_layout();
        let b1 = render(&layout).expect("render 1");
        let b2 = render(&layout).expect("render 2");
        assert_eq!(b1, b2);
    }

    #[test]
    fn render_er_is_deterministic() {
        let layout = er_layout();
        let b1 = render(&layout).expect("render 1");
        let b2 = render(&layout).expect("render 2");
        assert_eq!(b1, b2);
    }

    #[test]
    fn class_render_dangling_relation_is_error() {
        let layout = class_layout();
        let SemanticLayout::Class(mut cl) = layout else {
            panic!("expected class layout");
        };
        cl.relations[0].to = "does-not-exist".to_string();
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
            xml_escape("<b>bold & \"quotes\" 'ap'</b>"),
            "&lt;b&gt;bold &amp; &quot;quotes&quot; &apos;ap&apos;&lt;/b&gt;"
        );
    }

    #[test]
    fn xml_escape_japanese_passthrough() {
        assert_eq!(xml_escape("入力"), "入力");
    }

    // --- EMU conversion ---

    #[test]
    fn emu_pos_applies_margin() {
        // (0 + 20) * 9525 = 190500
        assert_eq!(emu_pos(0.0), 190500);
    }

    #[test]
    fn emu_len_no_margin() {
        assert_eq!(emu_len(0.0), 0);
        assert_eq!(emu_len(100.0), 952500);
    }

    // --- render() top-level: bytes are a valid ZIP for each diagram kind ---

    fn assert_valid_zip_shape(bytes: &[u8]) {
        assert!(
            bytes.starts_with(b"PK\x03\x04"),
            "pptx bytes must start with a ZIP local file header signature"
        );
        assert!(
            bytes.windows(4).any(|w| w == b"PK\x05\x06"),
            "pptx bytes must contain an End Of Central Directory signature"
        );
        assert!(!bytes.is_empty());
    }

    #[test]
    fn render_graph_produces_valid_zip() {
        let layout = graph_two_node_layout();
        let bytes = render(&layout).expect("graph render");
        assert_valid_zip_shape(&bytes);
    }

    #[test]
    fn render_state_produces_valid_zip() {
        let layout = state_basic_layout();
        let bytes = render(&layout).expect("state render");
        assert_valid_zip_shape(&bytes);
    }

    #[test]
    fn render_sequence_produces_valid_zip() {
        let layout = seq_layout();
        let bytes = render(&layout).expect("sequence render");
        assert_valid_zip_shape(&bytes);
    }

    // --- determinism ---

    #[test]
    fn render_graph_is_deterministic() {
        let layout = graph_two_node_layout();
        let b1 = render(&layout).expect("render 1");
        let b2 = render(&layout).expect("render 2");
        assert_eq!(b1, b2, "same input must produce byte-identical output");
    }

    #[test]
    fn render_state_is_deterministic() {
        let layout = state_basic_layout();
        let b1 = render(&layout).expect("render 1");
        let b2 = render(&layout).expect("render 2");
        assert_eq!(b1, b2);
    }

    #[test]
    fn render_sequence_is_deterministic() {
        let layout = seq_layout();
        let b1 = render(&layout).expect("render 1");
        let b2 = render(&layout).expect("render 2");
        assert_eq!(b1, b2);
    }

    // --- slide1.xml content checks (STORE means the raw text is present verbatim) ---

    #[test]
    fn graph_slide_contains_node_labels_and_shapes() {
        let layout = graph_two_node_layout();
        let bytes = render(&layout).expect("render");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("<p:sp>"), "must contain rectangle shapes");
        assert!(text.contains("<p:cxnSp>"), "must contain a connector");
        assert!(
            text.contains("Alpha"),
            "node label must appear in slide1.xml"
        );
        assert!(
            text.contains("Beta"),
            "node label must appear in slide1.xml"
        );
    }

    #[test]
    fn graph_node_label_is_xml_escaped() {
        use kozue_ir::{Diagram, Direction, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes
            .insert("x".into(), Node::new("x", "A < B & C \"quoted\""));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let bytes = render(&out.semantic).expect("render");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("&lt;"), "< must be escaped");
        assert!(text.contains("&amp;"), "& must be escaped");
        assert!(text.contains("&quot;"), "\" must be escaped");
    }

    #[test]
    fn graph_edge_label_is_rendered() {
        // graph_two_node_layout has no edge label; build one that does.
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        g.edges
            .push(Edge::new("a", "b", Some("yes".into()), ArrowType::Triangle));
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        let bytes = render(&out.semantic).expect("render");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("<a:t>yes</a:t>"),
            "edge label must be rendered in a label box"
        );
    }

    #[test]
    fn self_transition_uses_polyline_custgeom() {
        // A self-loop transition has a bent route (out-and-back), so it must be
        // emitted as a custGeom polyline rather than collapsing to a line.
        use kozue_ir::{Diagram, Endpoint, State, StateDiagram, Transition};
        let mut sd = StateDiagram::new();
        sd.states
            .insert("active".into(), State::new("active", "Active"));
        sd.transitions.push(Transition::new(
            Endpoint::State("active".into()),
            Endpoint::State("active".into()),
            Some("tick".into()),
        ));
        let out = kozue_layout::layout_full(&Diagram::State(sd)).expect("layout");
        let bytes = render(&out.semantic).expect("render");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("<a:custGeom>"),
            "self-loop must render as a custGeom polyline, not a straight line"
        );
        assert!(
            text.contains("<a:t>tick</a:t>"),
            "self-loop label must be rendered"
        );
    }

    #[test]
    fn state_slide_contains_initial_and_final_ellipses() {
        let layout = state_basic_layout();
        let bytes = render(&layout).expect("render");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("prst=\"ellipse\""),
            "pseudostates are ellipses"
        );
        assert!(text.contains("Idle"));
        assert!(text.contains("Active"));
        // Transition labels are rendered as white-filled label boxes on top of
        // the connector (state_basic_layout gives the idle->active edge "start").
        assert!(
            text.contains("start"),
            "transition label must appear in slide1.xml"
        );
    }

    #[test]
    fn sequence_slide_contains_participant_labels_and_lifelines() {
        let layout = seq_layout();
        let bytes = render(&layout).expect("render");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Alice"));
        assert!(text.contains("Bob"));
        assert!(
            text.contains("prstDash val=\"dash\""),
            "lifeline must be dashed"
        );
    }

    // --- error paths ---

    #[test]
    fn render_error_display_mentions_kind() {
        let e = RenderError::UnsupportedDiagram { kind: "sequence" };
        assert!(e.to_string().contains("sequence"));
    }

    #[test]
    fn dangling_edge_error_display() {
        let e = RenderError::DanglingEdge {
            node_id: "missing".to_string(),
        };
        assert!(e.to_string().contains("missing"));
    }

    #[test]
    fn unknown_endpoint_error_display() {
        let e = RenderError::UnknownEndpoint {
            description: "Initial pseudostate not present".to_string(),
        };
        assert!(e.to_string().contains("Initial"));
    }

    // --- zip content-types sanity: presentation part is declared ---

    #[test]
    fn pptx_declares_presentation_content_type() {
        let layout = graph_two_node_layout();
        let bytes = render(&layout).expect("render");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("presentationml.presentation.main+xml"),
            "Content_Types must declare the presentation part"
        );
        assert!(
            text.contains("presentationml.slide+xml"),
            "Content_Types must declare the slide part"
        );
    }
}
