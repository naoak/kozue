//! Deterministic Excalidraw (`.excalidraw` JSON) exporter for the SemanticLayout.
//!
//! ## Rationale
//!
//! Excalidraw is a popular hand-drawn-style whiteboard tool with an open,
//! well-documented JSON scene format. Like the draw.io exporter
//! (`kozue-render-drawio`), this crate reads the **semantic** layout produced
//! by [`kozue_layout::layout_full`] — not the flat [`Scene`](kozue_ir::Scene)
//! IR — so each Excalidraw element maps to a meaningful diagram element (node,
//! edge, pseudostate, etc.) rather than to a raw drawing primitive.
//!
//! ## Determinism
//!
//! Output is byte-identical for the same input on any platform:
//! - No `HashMap` anywhere; all collections use `Vec` (iteration order = layout order).
//! - Element IDs are deterministic strings: graph/state nodes `n{i}`, edges/transitions
//!   `e{i}`, bound text children `{ownerId}-text`, sequence lifelines `n{i}-lifeline`,
//!   state pseudostates `initial` / `final-outer` / `final-inner`.
//! - Excalidraw's `seed` (the hand-drawn "sketchiness" RNG seed) is normally
//!   randomized by the editor; here it is set deterministically to `index + 1`
//!   (1-based element position in the output array). This means re-rendering the
//!   same diagram always reproduces the exact same hand-drawn jitter — a
//!   deliberate, desirable trade-off for reproducible golden tests, at the cost
//!   of every kozue-exported drawing sharing a "look" for elements at the same
//!   position in the array.
//! - `versionNonce` and `updated` (Excalidraw's collaborative-editing
//!   bookkeeping fields, normally a random nonce and `Date.now()`) are omitted
//!   entirely: `restoreElement()` back-fills them on load, and synthesizing a
//!   random nonce or a wall-clock timestamp would violate determinism.
//! - Serialization uses `serde_json::to_string_pretty`, which is deterministic
//!   for a fixed input (field order follows Rust struct declaration order, not
//!   an unordered map).
//!
//! ## Coordinate space
//!
//! [`SemanticLayout`] coordinates use the Scene coordinate system: origin at
//! (0, 0), y-axis pointing down. This matches Excalidraw's canvas coordinate
//! system directly. A fixed 20 px margin is added on the exporter side
//! (matching the SVG / PNG / draw.io renderers) so nothing sits at the extreme
//! canvas edge.
//!
//! ## Supported diagram types
//!
//! - [`SemanticLayout::Graph`] — each node becomes a rounded rectangle with a
//!   bound text label; each edge becomes an arrow bound to its endpoint
//!   rectangles, with an optional bound text label.
//! - [`SemanticLayout::State`] — each named state becomes a rounded rectangle
//!   (as above); the initial pseudostate becomes a small filled ellipse; the
//!   final pseudostate becomes two concentric ellipses (an unfilled outer ring
//!   and a filled inner dot). Transitions become arrows bound to their
//!   endpoints (the outer ring for the final pseudostate).
//! - [`SemanticLayout::Sequence`] — each participant becomes a header
//!   rectangle (with bound text label) plus a separate dashed `line` element
//!   for the lifeline. Each message becomes an arrow; because a message can
//!   attach at any point along a lifeline (not just its header rectangle),
//!   message arrows are **not** Excalidraw-bound to a shape — their absolute
//!   `points` geometry is authoritative instead (see [`RenderError`] docs).
//!
//! - [`SemanticLayout::Class`] / [`SemanticLayout::Er`] — each
//!   [`CompartmentBox`] becomes a rectangle, a title text (stereotype +
//!   name), a horizontal `line` divider per compartment, and one free
//!   (unbound) text element per compartment listing its rows. Each
//!   [`RelationLayout`] becomes an arrow; because Excalidraw's arrowhead set
//!   is much smaller than kozue's [`EndMarker`] set, markers are approximated
//!   (see [`excalidraw_arrowhead`]) — this is a deliberate, documented
//!   degradation, not a silent one.
//!
//! Any future [`SemanticLayout`] variants return [`RenderError::UnsupportedDiagram`]
//! rather than silently dropping data.

use kozue_ir::{ArrowType, EndMarker, LineStyle, LineWeight, NodeKind, ParticipantKind};
use kozue_layout::semantic::{
    ClassLayout, GraphLayout, Point, SemanticLayout, SequenceLayout, StateEndpointId, StateLayout,
};
use kozue_layout::ExportInput;
use serde::Serialize;

const MARGIN: f64 = 20.0;
const STROKE_COLOR: &str = "#1e1e1e";
const FONT_SIZE: f64 = 20.0;
const FONT_FAMILY: u8 = 1;
const LINE_HEIGHT: f64 = 1.25;

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

/// An error that prevents a [`SemanticLayout`] from being exported to Excalidraw.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    /// The diagram type is not supported by this exporter. Returns an
    /// explicit error instead of silently dropping data.
    UnsupportedDiagram {
        /// Human-readable description of the unsupported variant.
        kind: &'static str,
    },
    /// A graph edge or sequence message references a node/participant ID that
    /// is not present in the layout. Silently dropping dangling edges would
    /// produce misleading output.
    DanglingEdge {
        /// The missing node/participant ID.
        node_id: String,
    },
    /// A state transition references an endpoint that cannot be resolved to
    /// an element. This covers unknown `StateEndpointId` variants added in
    /// the future (the type is `#[non_exhaustive]`).
    UnknownEndpoint {
        /// Human-readable description of the unresolved endpoint.
        description: String,
    },
    /// A future graph node kind has no defined Excalidraw mapping.
    UnknownNodeKind { description: String },
    /// A future semantic enum variant has no defined export mapping.
    InvalidSemantic { description: String },
    /// JSON serialization failed (e.g. a non-finite coordinate produced a
    /// NaN/Infinity float, which `serde_json` refuses to encode). This should
    /// not occur for layouts produced by `kozue_layout::layout_full`; it is
    /// kept as an explicit error rather than a panic.
    Serialization {
        /// The underlying `serde_json` error message.
        message: String,
    },
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::UnsupportedDiagram { kind } => {
                write!(f, "Excalidraw export does not support {kind} diagrams")
            }
            RenderError::DanglingEdge { node_id } => {
                write!(
                    f,
                    "Excalidraw export: edge references unknown node \"{node_id}\""
                )
            }
            RenderError::UnknownEndpoint { description } => {
                write!(
                    f,
                    "Excalidraw export: cannot resolve transition endpoint: {description}"
                )
            }
            RenderError::UnknownNodeKind { description } => {
                write!(
                    f,
                    "Excalidraw export: unknown graph node kind: {description}"
                )
            }
            RenderError::InvalidSemantic { description } => {
                write!(
                    f,
                    "Excalidraw export: invalid semantic value: {description}"
                )
            }
            RenderError::Serialization { message } => {
                write!(f, "Excalidraw export: JSON serialization failed: {message}")
            }
        }
    }
}

impl std::error::Error for RenderError {}

// ---------------------------------------------------------------------------
// Excalidraw JSON schema (subset needed for kozue's exported elements)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ExportedDataState {
    #[serde(rename = "type")]
    kind: &'static str,
    version: u32,
    source: &'static str,
    elements: Vec<AnyElement>,
    #[serde(rename = "appState")]
    app_state: AppState,
    files: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize)]
struct AppState {
    #[serde(rename = "gridSize")]
    grid_size: Option<u32>,
    #[serde(rename = "viewBackgroundColor")]
    view_background_color: &'static str,
}

/// `roundness` is `null` for every element except rounded rectangles, which
/// use Excalidraw's "adaptive radius" kind (`3`).
#[derive(Serialize)]
struct Roundness {
    #[serde(rename = "type")]
    kind: u8,
}

/// An entry in an element's `boundElements` array: a two-way link to a bound
/// text label or a bound arrow.
#[derive(Serialize, Clone, PartialEq)]
struct BoundElementRef {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

/// A legacy-form arrow binding (`focus`/`gap`). Accepted and normalized by
/// Excalidraw's `restore()`; `focus: 0.0` is deterministic and points the
/// binding at the endpoint shape's center-facing edge.
#[derive(Serialize)]
struct Binding {
    #[serde(rename = "elementId")]
    element_id: String,
    focus: f64,
    gap: f64,
}

/// Fields common to every Excalidraw element, emitted explicitly so
/// `restoreElement()` never has to guess geometry or style.
#[derive(Serialize)]
struct ElementBase<T> {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    angle: f64,
    #[serde(rename = "strokeColor")]
    stroke_color: &'static str,
    #[serde(rename = "backgroundColor")]
    background_color: &'static str,
    #[serde(rename = "fillStyle")]
    fill_style: &'static str,
    #[serde(rename = "strokeWidth")]
    stroke_width: f64,
    #[serde(rename = "strokeStyle")]
    stroke_style: &'static str,
    roughness: u8,
    opacity: u8,
    #[serde(rename = "groupIds")]
    group_ids: Vec<String>,
    roundness: Option<Roundness>,
    /// Deterministic hand-drawn-jitter seed; back-filled by [`assign_seeds`]
    /// to `index + 1` after all elements are built (see module docs).
    seed: u64,
    version: u32,
    #[serde(rename = "isDeleted")]
    is_deleted: bool,
    #[serde(rename = "boundElements")]
    bound_elements: Option<Vec<BoundElementRef>>,
    link: Option<String>,
    locked: bool,
    #[serde(flatten)]
    extra: T,
}

/// No type-specific fields (rectangles, ellipses).
#[derive(Serialize)]
struct ShapeExtra {}

#[derive(Serialize)]
struct TextExtra {
    #[serde(rename = "containerId")]
    container_id: Option<String>,
    text: String,
    #[serde(rename = "originalText")]
    original_text: String,
    #[serde(rename = "fontSize")]
    font_size: f64,
    #[serde(rename = "fontFamily")]
    font_family: u8,
    #[serde(rename = "textAlign")]
    text_align: &'static str,
    #[serde(rename = "verticalAlign")]
    vertical_align: &'static str,
    #[serde(rename = "lineHeight")]
    line_height: f64,
}

#[derive(Serialize)]
struct ArrowExtra {
    points: Vec<[f64; 2]>,
    #[serde(rename = "startBinding")]
    start_binding: Option<Binding>,
    #[serde(rename = "endBinding")]
    end_binding: Option<Binding>,
    #[serde(rename = "startArrowhead")]
    start_arrowhead: Option<&'static str>,
    #[serde(rename = "endArrowhead")]
    end_arrowhead: Option<&'static str>,
}

#[derive(Serialize)]
struct LineExtra {
    points: Vec<[f64; 2]>,
}

/// A kozue-exported element is always one of these four Excalidraw element
/// kinds. `#[serde(untagged)]` serializes the inner value directly (no extra
/// wrapper key), so each variant's `kind: "rectangle" | "ellipse" | "arrow" |
/// "line" | "text"` field (set by the constructor) is what Excalidraw sees as
/// the element's `type`.
#[derive(Serialize)]
#[serde(untagged)]
enum AnyElement {
    Shape(ElementBase<ShapeExtra>),
    Text(ElementBase<TextExtra>),
    Arrow(ElementBase<ArrowExtra>),
    Line(ElementBase<LineExtra>),
}

impl AnyElement {
    fn id(&self) -> &str {
        match self {
            AnyElement::Shape(e) => &e.id,
            AnyElement::Text(e) => &e.id,
            AnyElement::Arrow(e) => &e.id,
            AnyElement::Line(e) => &e.id,
        }
    }

    fn bound_elements_mut(&mut self) -> &mut Option<Vec<BoundElementRef>> {
        match self {
            AnyElement::Shape(e) => &mut e.bound_elements,
            AnyElement::Text(e) => &mut e.bound_elements,
            AnyElement::Arrow(e) => &mut e.bound_elements,
            AnyElement::Line(e) => &mut e.bound_elements,
        }
    }

    fn seed_mut(&mut self) -> &mut u64 {
        match self {
            AnyElement::Shape(e) => &mut e.seed,
            AnyElement::Text(e) => &mut e.seed,
            AnyElement::Arrow(e) => &mut e.seed,
            AnyElement::Line(e) => &mut e.seed,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Export a [`SemanticLayout`] to an Excalidraw scene JSON string
/// (`serde_json::to_string_pretty`-formatted).
///
/// Returns byte-identical output for the same input on any target (see
/// module docs for the determinism guarantees).
///
/// # Errors
///
/// Returns [`RenderError::UnsupportedDiagram`] for any future layout variants
/// that have no Excalidraw representation yet.
/// Returns [`RenderError::DanglingEdge`] if a graph edge or sequence message
/// references an unknown node/participant ID.
/// Returns [`RenderError::UnknownEndpoint`] if a state transition endpoint
/// cannot be resolved.
/// Returns [`RenderError::Serialization`] if the resulting scene cannot be
/// encoded as JSON (should not happen for finite layout coordinates).
pub fn render(layout: &SemanticLayout) -> Result<String, RenderError> {
    kozue_layout::validate_export_semantics(layout).map_err(|error| {
        RenderError::InvalidSemantic {
            description: error.to_string(),
        }
    })?;
    let mut elements = match layout {
        SemanticLayout::Graph(g) => render_graph(g)?,
        SemanticLayout::State(s) => render_state(s)?,
        SemanticLayout::Sequence(seq) => render_sequence(seq)?,
        SemanticLayout::Class(c) => render_class(c)?,
        SemanticLayout::Er(e) => render_er(e)?,
        _ => return Err(RenderError::UnsupportedDiagram { kind: "unknown" }),
    };
    assign_seeds(&mut elements);

    let doc = ExportedDataState {
        kind: "excalidraw",
        version: 2,
        source: "kozue",
        elements,
        app_state: AppState {
            grid_size: None,
            view_background_color: "#ffffff",
        },
        files: serde_json::Map::new(),
    };
    serde_json::to_string_pretty(&doc).map_err(|e| RenderError::Serialization {
        message: e.to_string(),
    })
}

/// Export a validated diagram/scene/semantic contract to Excalidraw JSON.
pub fn render_export(input: &ExportInput<'_>) -> Result<String, RenderError> {
    render(input.semantic())
}

/// Assign each element's `seed` to its 1-based position in the output array
/// (see module docs on determinism). Called once, after all elements for a
/// diagram have been built, so seeds are stable regardless of construction
/// order within a single node/edge (rectangle before its bound text, etc.).
fn assign_seeds(elements: &mut [AnyElement]) {
    for (i, el) in elements.iter_mut().enumerate() {
        *el.seed_mut() = (i + 1) as u64;
    }
}

/// Add `r` to the `boundElements` of the element with id `owner_id`, if not
/// already present (arrows bound to both endpoints of a self-loop would
/// otherwise be registered twice on the same node).
fn add_bound_element(elements: &mut [AnyElement], owner_id: &str, r: BoundElementRef) {
    for el in elements.iter_mut() {
        if el.id() != owner_id {
            continue;
        }
        let bound = el.bound_elements_mut();
        match bound {
            Some(v) => {
                if !v.contains(&r) {
                    v.push(r);
                }
            }
            None => *bound = Some(vec![r]),
        }
        return;
    }
}

// ---------------------------------------------------------------------------
// Element constructors
// ---------------------------------------------------------------------------

fn make_rect(id: &str, x: f64, y: f64, w: f64, h: f64) -> ElementBase<ShapeExtra> {
    make_rect_with_roundness(id, x, y, w, h, Some(Roundness { kind: 3 }))
}

fn make_rect_with_roundness(
    id: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    roundness: Option<Roundness>,
) -> ElementBase<ShapeExtra> {
    ElementBase {
        id: id.to_string(),
        kind: "rectangle",
        x,
        y,
        width: w,
        height: h,
        angle: 0.0,
        stroke_color: STROKE_COLOR,
        background_color: "transparent",
        fill_style: "hachure",
        stroke_width: 1.0,
        stroke_style: "solid",
        roughness: 1,
        opacity: 100,
        group_ids: Vec::new(),
        roundness,
        seed: 0,
        version: 1,
        is_deleted: false,
        bound_elements: None,
        link: None,
        locked: false,
        extra: ShapeExtra {},
    }
}

fn make_ellipse(
    id: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    background_color: &'static str,
    fill_style: &'static str,
) -> ElementBase<ShapeExtra> {
    ElementBase {
        id: id.to_string(),
        kind: "ellipse",
        x,
        y,
        width: w,
        height: h,
        angle: 0.0,
        stroke_color: STROKE_COLOR,
        background_color,
        fill_style,
        stroke_width: 1.0,
        stroke_style: "solid",
        roughness: 1,
        opacity: 100,
        group_ids: Vec::new(),
        roundness: None,
        seed: 0,
        version: 1,
        is_deleted: false,
        bound_elements: None,
        link: None,
        locked: false,
        extra: ShapeExtra {},
    }
}

/// A bound text label, roughly centered in its container's box. Excalidraw
/// re-centers text on load, but a sane initial position avoids a visible
/// jump before the first re-layout.
fn make_text(
    id: &str,
    container_id: Option<String>,
    text: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> ElementBase<TextExtra> {
    make_text_aligned(id, container_id, text, x, y, w, h, "center", "middle")
}

/// Like [`make_text`], but with an explicit alignment. Used for class/ER
/// compartment rows, which are left/top-aligned rather than centered.
#[allow(clippy::too_many_arguments)]
fn make_text_aligned(
    id: &str,
    container_id: Option<String>,
    text: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    text_align: &'static str,
    vertical_align: &'static str,
) -> ElementBase<TextExtra> {
    ElementBase {
        id: id.to_string(),
        kind: "text",
        x,
        y,
        width: w,
        height: h,
        angle: 0.0,
        stroke_color: STROKE_COLOR,
        background_color: "transparent",
        fill_style: "hachure",
        stroke_width: 1.0,
        stroke_style: "solid",
        roughness: 1,
        opacity: 100,
        group_ids: Vec::new(),
        roundness: None,
        seed: 0,
        version: 1,
        is_deleted: false,
        bound_elements: None,
        link: None,
        locked: false,
        extra: TextExtra {
            container_id,
            text: text.to_string(),
            original_text: text.to_string(),
            font_size: FONT_SIZE,
            font_family: FONT_FAMILY,
            text_align,
            vertical_align,
            line_height: LINE_HEIGHT,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn make_arrow(
    id: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    points: Vec<[f64; 2]>,
    start_binding: Option<Binding>,
    end_binding: Option<Binding>,
    start_arrowhead: Option<&'static str>,
    end_arrowhead: Option<&'static str>,
    stroke_style: &'static str,
    stroke_width: f64,
) -> ElementBase<ArrowExtra> {
    ElementBase {
        id: id.to_string(),
        kind: "arrow",
        x,
        y,
        width: w,
        height: h,
        angle: 0.0,
        stroke_color: STROKE_COLOR,
        background_color: "transparent",
        fill_style: "hachure",
        stroke_width,
        stroke_style,
        roughness: 1,
        opacity: 100,
        group_ids: Vec::new(),
        roundness: None,
        seed: 0,
        version: 1,
        is_deleted: false,
        bound_elements: None,
        link: None,
        locked: false,
        extra: ArrowExtra {
            points,
            start_binding,
            end_binding,
            start_arrowhead,
            end_arrowhead,
        },
    }
}

fn make_line(
    id: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    points: Vec<[f64; 2]>,
    dashed: bool,
) -> ElementBase<LineExtra> {
    ElementBase {
        id: id.to_string(),
        kind: "line",
        x,
        y,
        width: w,
        height: h,
        angle: 0.0,
        stroke_color: STROKE_COLOR,
        background_color: "transparent",
        fill_style: "hachure",
        stroke_width: 1.0,
        stroke_style: if dashed { "dashed" } else { "solid" },
        roughness: 1,
        opacity: 100,
        group_ids: Vec::new(),
        roundness: None,
        seed: 0,
        version: 1,
        is_deleted: false,
        bound_elements: None,
        link: None,
        locked: false,
        extra: LineExtra { points },
    }
}

// ---------------------------------------------------------------------------
// Geometry helper
// ---------------------------------------------------------------------------

/// Compute an arrow/line element's `(x, y, points, width, height)` from a
/// scene-space route: `x, y` is the margin-shifted first route point;
/// `points` are the remaining route points translated so the first entry is
/// always `[0, 0]`; `width`/`height` are the bounding box of those relative
/// points (not just the endpoint-to-endpoint span, so folded self-loop routes
/// get a geometry box that actually encloses the fold).
fn route_geometry(route: &[Point]) -> (f64, f64, Vec<[f64; 2]>, f64, f64) {
    debug_assert!(!route.is_empty(), "route must have at least one point");
    let x0 = route.first().map(|p| p.x).unwrap_or(0.0) + MARGIN;
    let y0 = route.first().map(|p| p.y).unwrap_or(0.0) + MARGIN;
    let points: Vec<[f64; 2]> = route
        .iter()
        .map(|p| [p.x + MARGIN - x0, p.y + MARGIN - y0])
        .collect();
    let max_x = points.iter().map(|p| p[0]).fold(0.0_f64, f64::max);
    let min_x = points.iter().map(|p| p[0]).fold(0.0_f64, f64::min);
    let max_y = points.iter().map(|p| p[1]).fold(0.0_f64, f64::max);
    let min_y = points.iter().map(|p| p[1]).fold(0.0_f64, f64::min);
    (x0, y0, points, max_x - min_x, max_y - min_y)
}

/// Heuristic bound-text size in the absence of real font metrics: width scales
/// with character count, height is a single line at [`FONT_SIZE`] /
/// [`LINE_HEIGHT`]. Excalidraw recomputes exact metrics on load; this only
/// needs to be a reasonable placeholder so the label doesn't render wildly
/// oversized or clipped before that happens.
fn text_size(label: &str) -> (f64, f64) {
    let w = (label.chars().count().max(1) as f64) * FONT_SIZE * 0.6;
    let h = FONT_SIZE * LINE_HEIGHT;
    (w, h)
}

// ---------------------------------------------------------------------------
// Graph diagram renderer
// ---------------------------------------------------------------------------

fn render_graph(g: &GraphLayout) -> Result<Vec<AnyElement>, RenderError> {
    let mut elements: Vec<AnyElement> = Vec::new();

    // Containers -- dashed, unfilled backdrop rectangle + a free (unbound)
    // top-left label text, in pre-order (matching `GraphLayout::containers`)
    // so they draw behind the nodes/edges emitted below.
    for (j, c) in g.containers.iter().enumerate() {
        let r = &c.rect;
        let rect_id = format!("c{j}");
        let mut rect = make_rect_with_roundness(
            &rect_id,
            r.x + MARGIN,
            r.y + MARGIN,
            r.width,
            r.height,
            None,
        );
        rect.stroke_style = "dashed";
        elements.push(AnyElement::Shape(rect));

        if let Some(label) = &c.label {
            let (tw, th) = text_size(label);
            elements.push(AnyElement::Text(make_text_aligned(
                &format!("{rect_id}-label"),
                None,
                label,
                r.x + MARGIN + 6.0,
                r.y + MARGIN + 4.0,
                tw,
                th,
                "left",
                "top",
            )));
        }
    }

    // Nodes -- rounded rectangle + bound text label (display label, not id).
    for (i, node) in g.nodes.iter().enumerate() {
        let r = &node.rect;
        let rect_id = format!("n{i}");
        let text_id = format!("{rect_id}-text");

        let (shape_kind, roundness) = match &node.kind {
            NodeKind::Default | NodeKind::RoundedRectangle => {
                ("rectangle", Some(Roundness { kind: 3 }))
            }
            NodeKind::Rectangle => ("rectangle", None),
            NodeKind::Circle => ("ellipse", None),
            NodeKind::Diamond => ("diamond", None),
            kind => {
                return Err(RenderError::UnknownNodeKind {
                    description: format!("{kind:?}"),
                })
            }
        };
        let mut rect = make_rect_with_roundness(
            &rect_id,
            r.x + MARGIN,
            r.y + MARGIN,
            r.width,
            r.height,
            roundness,
        );
        rect.kind = shape_kind;
        rect.bound_elements = Some(vec![BoundElementRef {
            id: text_id.clone(),
            kind: "text",
        }]);
        elements.push(AnyElement::Shape(rect));

        let (tw, th) = text_size(&node.label);
        let tx = r.x + MARGIN + r.width / 2.0 - tw / 2.0;
        let ty = r.y + MARGIN + r.height / 2.0 - th / 2.0;
        elements.push(AnyElement::Text(make_text(
            &text_id,
            Some(rect_id),
            &node.label,
            tx,
            ty,
            tw,
            th,
        )));
    }

    // Node id -> index lookup (Vec-based, deterministic). Returns
    // RenderError::DanglingEdge for unknown IDs instead of silently emitting
    // a binding to a nonexistent element.
    let find_node_idx =
        |id: &str| -> Option<usize> { g.nodes.iter().position(|n| n.id.as_str() == id) };

    // Edges -- arrow bound to both endpoint rectangles, plus an optional
    // bound text label.
    //
    // Compass ports (`Edge.from_port`/`to_port`, M3a4) are geometry-driven
    // here, not a separate code path: the layout engine already snapped
    // `edge.route`'s endpoints to the requested side via
    // `route_geometry`(below), so the emitted `points` array carries the port
    // faithfully with no silent drop. The `focus: 0.0` legacy-form binding
    // (below) is the same fixed representation choice already made for plain
    // shape-boundary clipping since M3a2a-II: Excalidraw's own `restore()` may
    // recompute a rendered arrow's visual endpoint from the bound shape's
    // perimeter using that binding, but that is a downstream Excalidraw
    // rendering behavior, not a kozue export decision -- the exported bytes
    // (`points`) always encode the exact snapped port location.
    for (i, edge) in g.edges.iter().enumerate() {
        let src_idx =
            find_node_idx(edge.from.id.as_str()).ok_or_else(|| RenderError::DanglingEdge {
                node_id: edge.from.id.to_string(),
            })?;
        let tgt_idx =
            find_node_idx(edge.to.id.as_str()).ok_or_else(|| RenderError::DanglingEdge {
                node_id: edge.to.id.to_string(),
            })?;
        let src_id = format!("n{src_idx}");
        let tgt_id = format!("n{tgt_idx}");
        let arrow_id = format!("e{i}");

        let (ax, ay, points, w, h) = route_geometry(&edge.route);
        // `ArrowType::Triangle` is kozue's only directed arrowhead and is drawn
        // as a filled triangle in the SVG/PNG backends; map it to Excalidraw's
        // `"triangle"` head (not the open `"arrow"`) so the export matches.
        let end_arrowhead = if edge.arrow == ArrowType::None {
            None
        } else {
            Some("triangle")
        };
        // Source-end marker: Excalidraw's open `"arrow"` head (there is no
        // separate filled-triangle *start* head in the format).
        let start_arrowhead = if edge.from_arrow == ArrowType::None {
            None
        } else {
            Some("arrow")
        };
        let stroke_style = match edge.line {
            LineStyle::Dashed => "dashed",
            LineStyle::Dotted => "dotted",
            _ => "solid",
        };
        let stroke_width = if edge.weight == LineWeight::Thick {
            2.0
        } else {
            1.0
        };
        let arrow = make_arrow(
            &arrow_id,
            ax,
            ay,
            w,
            h,
            points,
            Some(Binding {
                element_id: src_id.clone(),
                focus: 0.0,
                gap: 4.0,
            }),
            Some(Binding {
                element_id: tgt_id.clone(),
                focus: 0.0,
                gap: 4.0,
            }),
            start_arrowhead,
            end_arrowhead,
            stroke_style,
            stroke_width,
        );
        elements.push(AnyElement::Arrow(arrow));
        add_bound_element(
            &mut elements,
            &src_id,
            BoundElementRef {
                id: arrow_id.clone(),
                kind: "arrow",
            },
        );
        add_bound_element(
            &mut elements,
            &tgt_id,
            BoundElementRef {
                id: arrow_id.clone(),
                kind: "arrow",
            },
        );

        if let Some(label) = &edge.label {
            let label_id = format!("{arrow_id}-text");
            let (tw, th) = text_size(label);
            let (lx, ly) = match &edge.label_anchor {
                Some(p) => (p.x + MARGIN - tw / 2.0, p.y + MARGIN - th / 2.0),
                None => {
                    let mid = &edge.route[edge.route.len() / 2];
                    (mid.x + MARGIN - tw / 2.0, mid.y + MARGIN - th / 2.0)
                }
            };
            elements.push(AnyElement::Text(make_text(
                &label_id,
                Some(arrow_id.clone()),
                label,
                lx,
                ly,
                tw,
                th,
            )));
            add_bound_element(
                &mut elements,
                &arrow_id,
                BoundElementRef {
                    id: label_id,
                    kind: "text",
                },
            );
        }
    }

    Ok(elements)
}

// ---------------------------------------------------------------------------
// State diagram renderer
// ---------------------------------------------------------------------------

fn render_state(s: &StateLayout) -> Result<Vec<AnyElement>, RenderError> {
    let mut elements: Vec<AnyElement> = Vec::new();

    // Named state vertices -- rounded rectangle + bound text label.
    for (i, state) in s.states.iter().enumerate() {
        let r = &state.rect;
        let rect_id = format!("n{i}");
        let text_id = format!("{rect_id}-text");

        let mut rect = make_rect(&rect_id, r.x + MARGIN, r.y + MARGIN, r.width, r.height);
        rect.bound_elements = Some(vec![BoundElementRef {
            id: text_id.clone(),
            kind: "text",
        }]);
        elements.push(AnyElement::Shape(rect));

        let (tw, th) = text_size(&state.label);
        let tx = r.x + MARGIN + r.width / 2.0 - tw / 2.0;
        let ty = r.y + MARGIN + r.height / 2.0 - th / 2.0;
        elements.push(AnyElement::Text(make_text(
            &text_id,
            Some(rect_id),
            &state.label,
            tx,
            ty,
            tw,
            th,
        )));
    }

    // Initial pseudostate: a small filled ellipse, id "initial".
    if let Some(init) = &s.initial {
        let cx = init.center.x + MARGIN;
        let cy = init.center.y + MARGIN;
        let r = init.radius;
        elements.push(AnyElement::Shape(make_ellipse(
            "initial",
            cx - r,
            cy - r,
            r * 2.0,
            r * 2.0,
            STROKE_COLOR,
            "solid",
        )));
    }

    // Final pseudostate: an unfilled outer ring ("final-outer", the
    // connectable element transitions target) plus a filled inner dot
    // ("final-inner", decorative, drawn on top). Concentric at `final.center`,
    // matching the draw.io / SVG renderers' two-ellipse construction.
    if let Some(fin) = &s.final_state {
        let cx = fin.center.x + MARGIN;
        let cy = fin.center.y + MARGIN;
        let ro = fin.outer_radius;
        let ri = fin.inner_radius;
        elements.push(AnyElement::Shape(make_ellipse(
            "final-outer",
            cx - ro,
            cy - ro,
            ro * 2.0,
            ro * 2.0,
            "transparent",
            "hachure",
        )));
        elements.push(AnyElement::Shape(make_ellipse(
            "final-inner",
            cx - ri,
            cy - ri,
            ri * 2.0,
            ri * 2.0,
            STROKE_COLOR,
            "solid",
        )));
    }

    // State endpoint id -> element id lookup (Vec-based, deterministic).
    // Returns RenderError::UnknownEndpoint for unknown StateEndpointId
    // variants instead of silently emitting an arrow with a missing endpoint.
    let state_element_id = |ep: &StateEndpointId| -> Result<String, RenderError> {
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
                    Ok("final-outer".to_string())
                } else {
                    Err(RenderError::UnknownEndpoint {
                        description: "Final pseudostate referenced but not present in layout"
                            .to_string(),
                    })
                }
            }
            // Any future non_exhaustive variant -- refuse rather than
            // silently produce an arrow with a missing binding.
            _ => Err(RenderError::UnknownEndpoint {
                description: format!("unrecognised StateEndpointId variant: {ep:?}"),
            }),
        }
    };

    // Transitions -- arrow bound to both endpoints. Self-loops need no
    // special-casing here: `tr.route` already contains the fold waypoints
    // computed by the layout engine, and `route_geometry` turns any route
    // (straight or folded) into the arrow's absolute `points` polyline.
    for (i, tr) in s.transitions.iter().enumerate() {
        let src_id = state_element_id(&tr.from)?;
        let tgt_id = state_element_id(&tr.to)?;
        let arrow_id = format!("e{i}");

        let (ax, ay, points, w, h) = route_geometry(&tr.route);
        let arrow = make_arrow(
            &arrow_id,
            ax,
            ay,
            w,
            h,
            points,
            Some(Binding {
                element_id: src_id.clone(),
                focus: 0.0,
                gap: 4.0,
            }),
            Some(Binding {
                element_id: tgt_id.clone(),
                focus: 0.0,
                gap: 4.0,
            }),
            None,
            // Filled triangle head, matching the SVG/PNG backends.
            Some("triangle"),
            "solid",
            1.0,
        );
        elements.push(AnyElement::Arrow(arrow));
        add_bound_element(
            &mut elements,
            &src_id,
            BoundElementRef {
                id: arrow_id.clone(),
                kind: "arrow",
            },
        );
        add_bound_element(
            &mut elements,
            &tgt_id,
            BoundElementRef {
                id: arrow_id.clone(),
                kind: "arrow",
            },
        );

        if let Some(label) = &tr.label {
            let label_id = format!("{arrow_id}-text");
            let (tw, th) = text_size(label);
            let (lx, ly) = match &tr.label_anchor {
                Some(p) => (p.x + MARGIN - tw / 2.0, p.y + MARGIN - th / 2.0),
                None => {
                    let mid = &tr.route[tr.route.len() / 2];
                    (mid.x + MARGIN - tw / 2.0, mid.y + MARGIN - th / 2.0)
                }
            };
            elements.push(AnyElement::Text(make_text(
                &label_id,
                Some(arrow_id.clone()),
                label,
                lx,
                ly,
                tw,
                th,
            )));
            add_bound_element(
                &mut elements,
                &arrow_id,
                BoundElementRef {
                    id: label_id,
                    kind: "text",
                },
            );
        }
    }

    Ok(elements)
}

// ---------------------------------------------------------------------------
// Sequence diagram renderer
// ---------------------------------------------------------------------------

fn render_sequence(s: &SequenceLayout) -> Result<Vec<AnyElement>, RenderError> {
    let mut elements: Vec<AnyElement> = Vec::new();

    // Participants -- header rectangle (+ bound text label) plus a separate
    // dashed `line` element for the lifeline.
    for (i, p) in s.participants.iter().enumerate() {
        let r = &p.header_rect;
        let rect_id = format!("n{i}");
        let text_id = format!("{rect_id}-text");
        let lifeline_id = format!("{rect_id}-lifeline");
        // Header and lifeline share a deterministic group so that dragging the
        // participant in Excalidraw moves the whole column together. The header's
        // bound text follows its container automatically, so it needs no group.
        let group_id = format!("participant-{i}");

        let mut rect = make_rect(&rect_id, r.x + MARGIN, r.y + MARGIN, r.width, r.height);
        rect.group_ids = vec![group_id.clone()];
        rect.bound_elements = Some(vec![BoundElementRef {
            id: text_id.clone(),
            kind: "text",
        }]);
        elements.push(AnyElement::Shape(rect));

        let st_label = match &p.kind {
            ParticipantKind::Default => None,
            ParticipantKind::Actor => Some("«actor»"),
            ParticipantKind::Boundary => Some("«boundary»"),
            ParticipantKind::Control => Some("«control»"),
            ParticipantKind::Entity => Some("«entity»"),
            ParticipantKind::Database => Some("«database»"),
            ParticipantKind::Collections => Some("«collections»"),
            ParticipantKind::Queue => Some("«queue»"),
            _ => None,
        };

        if let Some(st) = st_label {
            let (stw, sth) = text_size(st);
            let stx = r.x + MARGIN + r.width / 2.0 - stw / 2.0;
            let sty = r.y + MARGIN + r.height * 0.25 - sth / 2.0;
            let st_id = format!("{rect_id}-stereotype");
            elements.push(AnyElement::Text(make_text(
                &st_id, None, st, stx, sty, stw, sth,
            )));
        }

        let (tw, th) = text_size(&p.label);
        let tx = r.x + MARGIN + r.width / 2.0 - tw / 2.0;
        let ty = r.y + MARGIN + r.height / 2.0 - th / 2.0;
        elements.push(AnyElement::Text(make_text(
            &text_id,
            Some(rect_id),
            &p.label,
            tx,
            ty,
            tw,
            th,
        )));

        let lifeline_route = [
            Point::new(p.lifeline_x, p.lifeline_y0),
            Point::new(p.lifeline_x, p.lifeline_y1),
        ];
        let (lx, ly, points, w, h) = route_geometry(&lifeline_route);
        let mut lifeline = make_line(&lifeline_id, lx, ly, w, h, points, /* dashed */ true);
        lifeline.group_ids = vec![group_id.clone()];
        elements.push(AnyElement::Line(lifeline));
    }

    // Participant id -> index lookup (Vec-based, deterministic). Returns
    // DanglingEdge for an unknown participant instead of dropping the message.
    let find_participant =
        |id: &str| -> Option<usize> { s.participants.iter().position(|p| p.id.as_str() == id) };

    // Messages -- arrow with absolute `points` geometry. Unlike graph edges
    // and state transitions, a message can attach at any y along a lifeline
    // (not just the header rectangle), so binding to a shape is not
    // meaningful here: both `startBinding`/`endBinding` stay `null` and the
    // arrow's own `points` remain authoritative (see module docs).
    for (i, m) in s.messages.iter().enumerate() {
        find_participant(m.from.as_str()).ok_or_else(|| RenderError::DanglingEdge {
            node_id: m.from.to_string(),
        })?;
        find_participant(m.to.as_str()).ok_or_else(|| RenderError::DanglingEdge {
            node_id: m.to.to_string(),
        })?;

        let arrow_id = format!("e{i}");
        let (ax, ay, points, w, h) = route_geometry(&m.route);
        // Filled triangle head, matching the SVG/PNG backends.
        let end_arrowhead = if m.arrow == ArrowType::None {
            None
        } else {
            Some("triangle")
        };
        let stroke_style = if m.line == LineStyle::Dashed {
            "dashed"
        } else {
            "solid"
        };
        let arrow = make_arrow(
            &arrow_id,
            ax,
            ay,
            w,
            h,
            points,
            None,
            None,
            None,
            end_arrowhead,
            stroke_style,
            1.0,
        );
        elements.push(AnyElement::Arrow(arrow));

        if let Some(label) = &m.label {
            let label_id = format!("{arrow_id}-text");
            let (tw, th) = text_size(label);
            let (lx, ly) = match &m.label_anchor {
                Some(p) => (p.x + MARGIN - tw / 2.0, p.y + MARGIN - th / 2.0),
                None => {
                    let mid = &m.route[m.route.len() / 2];
                    (mid.x + MARGIN - tw / 2.0, mid.y + MARGIN - th / 2.0)
                }
            };
            elements.push(AnyElement::Text(make_text(
                &label_id,
                Some(arrow_id.clone()),
                label,
                lx,
                ly,
                tw,
                th,
            )));
            add_bound_element(
                &mut elements,
                &arrow_id,
                BoundElementRef {
                    id: label_id,
                    kind: "text",
                },
            );
        }
    }

    Ok(elements)
}

// ---------------------------------------------------------------------------
// Class / ER diagram renderer
// ---------------------------------------------------------------------------

/// Map an [`EndMarker`] to an Excalidraw arrowhead name.
///
/// Excalidraw's arrowhead set (`arrow` / `triangle` / `triangle_outline` /
/// `bar` / `dot` / `diamond` / `diamond_outline`) is much smaller than
/// kozue's ten [`EndMarker`] variants, so ER crow's-foot cardinalities are
/// lossily approximated here (documented in the crate's determinism/degradation
/// notes): the "many" markers collapse onto `triangle` and the "zero"
/// markers collapse onto `dot`, each losing the paired bar/crow they'd carry
/// in a full crow's-foot rendering.
///
/// | `EndMarker`      | Excalidraw arrowhead | notes                    |
/// |-------------------|-----------------------|---------------------------|
/// | `None`            | (none)                |                           |
/// | `HollowTriangle`  | `triangle_outline`    |                           |
/// | `OpenArrow`       | `arrow`               |                           |
/// | `FilledDiamond`   | `diamond`             |                           |
/// | `HollowDiamond`   | `diamond_outline`     |                           |
/// | `ErOne`           | `bar`                 |                           |
/// | `ErMany`          | `triangle`            | approximates the crow's foot |
/// | `ErZeroOrOne`      | `dot`                 | loses the paired bar     |
/// | `ErOneOrMany`      | `triangle`            | loses the paired bar     |
/// | `ErZeroOrMany`     | `dot`                 | loses the paired crow    |
fn excalidraw_arrowhead(marker: EndMarker) -> Option<&'static str> {
    match marker {
        EndMarker::None => None,
        EndMarker::HollowTriangle => Some("triangle_outline"),
        EndMarker::OpenArrow => Some("arrow"),
        EndMarker::FilledDiamond => Some("diamond"),
        EndMarker::HollowDiamond => Some("diamond_outline"),
        EndMarker::ErOne => Some("bar"),
        EndMarker::ErMany => Some("triangle"),
        EndMarker::ErZeroOrOne => Some("dot"),
        EndMarker::ErOneOrMany => Some("triangle"),
        EndMarker::ErZeroOrMany => Some("dot"),
        _ => None,
    }
}

fn render_class(layout: &ClassLayout) -> Result<Vec<AnyElement>, RenderError> {
    let mut elements: Vec<AnyElement> = Vec::new();

    // Boxes -- rectangle + centered title text (stereotype + name) + one
    // divider line and one left/top-aligned free text per compartment.
    for (i, b) in layout.boxes.iter().enumerate() {
        let r = &b.rect;
        let rect_id = format!("n{i}");
        elements.push(AnyElement::Shape(make_rect(
            &rect_id,
            r.x + MARGIN,
            r.y + MARGIN,
            r.width,
            r.height,
        )));

        let title_bottom = b
            .compartments
            .first()
            .map(|c| c.top_y)
            .unwrap_or(r.y + r.height);
        let title_h = (title_bottom - r.y).max(1.0);
        let mut title_lines: Vec<String> = Vec::new();
        if let Some(st) = &b.stereotype {
            title_lines.push(format!("\u{ab}{st}\u{bb}"));
        }
        title_lines.push(b.title.clone());
        let title_text = title_lines.join("\n");
        elements.push(AnyElement::Text(make_text(
            &format!("{rect_id}-title"),
            None,
            &title_text,
            r.x + MARGIN,
            r.y + MARGIN,
            r.width,
            title_h,
        )));

        for (ci, c) in b.compartments.iter().enumerate() {
            let div_id = format!("{rect_id}-div{ci}");
            let route = [Point::new(r.x, c.top_y), Point::new(r.x + r.width, c.top_y)];
            let (lx, ly, points, w, h) = route_geometry(&route);
            elements.push(AnyElement::Line(make_line(
                &div_id, lx, ly, w, h, points, false,
            )));

            let bottom = b
                .compartments
                .get(ci + 1)
                .map(|c2| c2.top_y)
                .unwrap_or(r.y + r.height);
            let sect_h = (bottom - c.top_y).max(1.0);
            let content = c.rows.join("\n");
            elements.push(AnyElement::Text(make_text_aligned(
                &format!("{rect_id}-sect{ci}"),
                None,
                &content,
                r.x + MARGIN + 4.0,
                c.top_y + MARGIN,
                r.width - 8.0,
                sect_h,
                "left",
                "top",
            )));
        }
    }

    // Box id -> index lookup (Vec-based, deterministic). Returns
    // RenderError::DanglingEdge for unknown IDs.
    let find_box =
        |id: &str| -> Option<usize> { layout.boxes.iter().position(|b| b.id.as_str() == id) };

    // Relations -- arrow bound to both endpoint rectangles, with markers on
    // both ends and optional label / multiplicity texts.
    for (i, rel) in layout.relations.iter().enumerate() {
        let src_idx = find_box(rel.from.as_str()).ok_or_else(|| RenderError::DanglingEdge {
            node_id: rel.from.to_string(),
        })?;
        let tgt_idx = find_box(rel.to.as_str()).ok_or_else(|| RenderError::DanglingEdge {
            node_id: rel.to.to_string(),
        })?;
        let src_id = format!("n{src_idx}");
        let tgt_id = format!("n{tgt_idx}");
        let arrow_id = format!("e{i}");

        let route: Vec<Point> = rel.points.iter().map(|&(x, y)| Point::new(x, y)).collect();
        let (ax, ay, points, w, h) = route_geometry(&route);
        let start_arrowhead = excalidraw_arrowhead(rel.from_marker);
        let end_arrowhead = excalidraw_arrowhead(rel.to_marker);
        let stroke_style = if rel.line == LineStyle::Dashed {
            "dashed"
        } else {
            "solid"
        };
        let arrow = make_arrow(
            &arrow_id,
            ax,
            ay,
            w,
            h,
            points,
            Some(Binding {
                element_id: src_id.clone(),
                focus: 0.0,
                gap: 4.0,
            }),
            Some(Binding {
                element_id: tgt_id.clone(),
                focus: 0.0,
                gap: 4.0,
            }),
            start_arrowhead,
            end_arrowhead,
            stroke_style,
            1.0,
        );
        elements.push(AnyElement::Arrow(arrow));
        add_bound_element(
            &mut elements,
            &src_id,
            BoundElementRef {
                id: arrow_id.clone(),
                kind: "arrow",
            },
        );
        add_bound_element(
            &mut elements,
            &tgt_id,
            BoundElementRef {
                id: arrow_id.clone(),
                kind: "arrow",
            },
        );

        if let Some(label) = &rel.label {
            let label_id = format!("{arrow_id}-text");
            let (tw, th) = text_size(label);
            let mid = &route[route.len() / 2];
            let lx = mid.x + MARGIN - tw / 2.0;
            let ly = mid.y + MARGIN - th / 2.0;
            elements.push(AnyElement::Text(make_text(
                &label_id,
                Some(arrow_id.clone()),
                label,
                lx,
                ly,
                tw,
                th,
            )));
            add_bound_element(
                &mut elements,
                &arrow_id,
                BoundElementRef {
                    id: label_id,
                    kind: "text",
                },
            );
        }

        // Multiplicity labels -- small free (unbound) texts just inside each
        // endpoint of the route.
        if let Some(m) = &rel.from_mult {
            let p0 = &route[0];
            let (tw, th) = text_size(m);
            elements.push(AnyElement::Text(make_text(
                &format!("{arrow_id}-mult-from"),
                None,
                m,
                p0.x + MARGIN + 4.0,
                p0.y + MARGIN - th - 2.0,
                tw,
                th,
            )));
        }
        if let Some(m) = &rel.to_mult {
            let p1 = &route[route.len() - 1];
            let (tw, th) = text_size(m);
            elements.push(AnyElement::Text(make_text(
                &format!("{arrow_id}-mult-to"),
                None,
                m,
                p1.x + MARGIN - tw - 4.0,
                p1.y + MARGIN - th - 2.0,
                tw,
                th,
            )));
        }
    }

    Ok(elements)
}

fn render_er(layout: &ClassLayout) -> Result<Vec<AnyElement>, RenderError> {
    // ER layouts are structurally identical ClassLayouts (see
    // `kozue_layout::semantic::ErLayout`); reuse the same renderer.
    render_class(layout)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

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

    // Helper: graph layout with an undirected edge (ArrowType::None).
    fn graph_undirected_layout() -> SemanticLayout {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::None));
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

    fn parse(json: &str) -> Value {
        serde_json::from_str(json).expect("output must be valid JSON")
    }

    // --- top-level envelope ---

    #[test]
    fn graph_render_produces_valid_envelope() {
        let layout = graph_two_node_layout();
        let json = render(&layout).expect("graph render");
        let v = parse(&json);
        assert_eq!(v["type"], "excalidraw");
        assert_eq!(v["version"], 2);
        assert!(!v["elements"].as_array().expect("elements array").is_empty());
    }

    // --- rectangle + bound text ---

    #[test]
    fn graph_render_node_rectangle_has_bound_text_label() {
        let layout = graph_two_node_layout();
        let json = render(&layout).expect("render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();

        let rect = elements
            .iter()
            .find(|e| e["id"] == "n0")
            .expect("n0 rectangle present");
        assert_eq!(rect["type"], "rectangle");
        let bound = rect["boundElements"].as_array().expect("boundElements");
        assert_eq!(bound[0]["id"], "n0-text");
        assert_eq!(bound[0]["type"], "text");

        let text = elements
            .iter()
            .find(|e| e["id"] == "n0-text")
            .expect("n0-text present");
        assert_eq!(text["type"], "text");
        assert_eq!(text["containerId"], "n0");
        // The label text must be the display label ("Alpha"), not the id ("a").
        assert_eq!(text["text"], "Alpha");
        assert_eq!(text["originalText"], "Alpha");
    }

    // --- arrow geometry + bindings ---

    #[test]
    fn graph_render_edge_arrow_geometry_and_bindings() {
        let layout = graph_two_node_layout();
        let json = render(&layout).expect("render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();

        let arrow = elements
            .iter()
            .find(|e| e["id"] == "e0")
            .expect("e0 arrow present");
        assert_eq!(arrow["type"], "arrow");
        let points = arrow["points"].as_array().expect("points array");
        assert_eq!(points[0], serde_json::json!([0.0, 0.0]));
        assert_eq!(arrow["startBinding"]["elementId"], "n0");
        assert_eq!(arrow["endBinding"]["elementId"], "n1");
        assert_eq!(arrow["endArrowhead"], "triangle");
        assert!(arrow["startArrowhead"].is_null());

        // Both endpoint rectangles must record the arrow as a bound element.
        let n0 = elements.iter().find(|e| e["id"] == "n0").unwrap();
        let n1 = elements.iter().find(|e| e["id"] == "n1").unwrap();
        let n0_bound: Vec<&str> = n0["boundElements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_str().unwrap())
            .collect();
        let n1_bound: Vec<&str> = n1["boundElements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_str().unwrap())
            .collect();
        assert!(n0_bound.contains(&"e0"));
        assert!(n1_bound.contains(&"e0"));
    }

    #[test]
    fn graph_render_undirected_edge_has_null_end_arrowhead() {
        let layout = graph_undirected_layout();
        let json = render(&layout).expect("render");
        let v = parse(&json);
        let arrow = v["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "e0")
            .unwrap();
        assert!(arrow["endArrowhead"].is_null());
    }

    // Helper: graph layout with a single edge carrying non-default
    // presentation (from_arrow/line/weight all set).
    fn graph_presentation_layout(
        from_arrow: kozue_ir::ArrowType,
        line: LineStyle,
        weight: LineWeight,
    ) -> SemanticLayout {
        use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        let mut e = Edge::new("a", "b", None, ArrowType::Triangle);
        e.from_arrow = from_arrow;
        e.line = line;
        e.weight = weight;
        g.edges.push(e);
        let out = kozue_layout::layout_full(&Diagram::Graph(g)).expect("layout");
        out.semantic
    }

    #[test]
    fn graph_render_from_arrow_maps_to_start_arrowhead_arrow() {
        let layout = graph_presentation_layout(
            kozue_ir::ArrowType::Triangle,
            LineStyle::Solid,
            LineWeight::Normal,
        );
        let json = render(&layout).expect("render");
        let v = parse(&json);
        let arrow = v["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "e0")
            .unwrap();
        assert_eq!(arrow["startArrowhead"], "arrow");
    }

    #[test]
    fn graph_render_line_maps_to_stroke_style() {
        for (line, expected) in [
            (LineStyle::Solid, "solid"),
            (LineStyle::Dashed, "dashed"),
            (LineStyle::Dotted, "dotted"),
        ] {
            let layout =
                graph_presentation_layout(kozue_ir::ArrowType::None, line, LineWeight::Normal);
            let json = render(&layout).expect("render");
            let v = parse(&json);
            let arrow = v["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "e0")
                .unwrap();
            assert_eq!(arrow["strokeStyle"], expected, "line {line:?}");
        }
    }

    #[test]
    fn graph_render_thick_weight_maps_to_stroke_width_two() {
        let layout = graph_presentation_layout(
            kozue_ir::ArrowType::None,
            LineStyle::Solid,
            LineWeight::Thick,
        );
        let json = render(&layout).expect("render");
        let v = parse(&json);
        let arrow = v["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "e0")
            .unwrap();
        assert_eq!(arrow["strokeWidth"], 2.0);
    }

    #[test]
    fn graph_render_default_presentation_edge_is_unchanged() {
        // Same fixture as `graph_render_edge_arrow_geometry_and_bindings`, just
        // asserting the two new fields keep their legacy defaults.
        let layout = graph_two_node_layout();
        let json = render(&layout).expect("render");
        let v = parse(&json);
        let arrow = v["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "e0")
            .unwrap();
        assert!(arrow["startArrowhead"].is_null());
        assert_eq!(arrow["strokeStyle"], "solid");
        assert_eq!(arrow["strokeWidth"], 1.0);
    }

    // --- M3a3: containers ---

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
    fn graph_with_no_containers_has_no_container_elements() {
        let layout = graph_two_node_layout();
        let json = render(&layout).expect("render");
        let v = parse(&json);
        assert!(
            !v["elements"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == "c0"),
            "no container element expected: {json}"
        );
    }

    #[test]
    fn graph_container_emits_dashed_rectangle_and_label_before_nodes() {
        let layout = graph_with_container_layout();
        let json = render(&layout).expect("render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();

        let rect = elements
            .iter()
            .find(|e| e["id"] == "c0")
            .expect("container rectangle element");
        assert_eq!(rect["type"], "rectangle");
        assert_eq!(rect["strokeStyle"], "dashed");
        assert_eq!(rect["backgroundColor"], "transparent");

        let label = elements
            .iter()
            .find(|e| e["id"] == "c0-label")
            .expect("container label text element");
        assert_eq!(label["type"], "text");
        assert_eq!(label["text"], "Group");

        let c0_index = elements.iter().position(|e| e["id"] == "c0").unwrap();
        let n0_index = elements.iter().position(|e| e["id"] == "n0").unwrap();
        assert!(
            c0_index < n0_index,
            "container element must be emitted before node elements"
        );
    }

    // --- determinism ---

    #[test]
    fn graph_render_is_deterministic() {
        let layout = graph_two_node_layout();
        let json1 = render(&layout).expect("render 1");
        let json2 = render(&layout).expect("render 2");
        assert_eq!(
            json1, json2,
            "same input must produce byte-identical output"
        );
    }

    #[test]
    fn state_render_is_deterministic() {
        let layout = state_basic_layout();
        let json1 = render(&layout).expect("render 1");
        let json2 = render(&layout).expect("render 2");
        assert_eq!(json1, json2, "state render must be deterministic");
    }

    #[test]
    fn sequence_render_is_deterministic() {
        let layout = seq_layout();
        let json1 = render(&layout).expect("render 1");
        let json2 = render(&layout).expect("render 2");
        assert_eq!(json1, json2, "sequence render must be deterministic");
    }

    // --- seed assignment ---

    #[test]
    fn seeds_are_index_plus_one_and_unique() {
        let layout = graph_two_node_layout();
        let json = render(&layout).expect("render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();
        for (i, el) in elements.iter().enumerate() {
            assert_eq!(el["seed"], (i + 1) as u64, "seed must be index+1");
        }
    }

    // --- state pseudostates ---

    #[test]
    fn state_render_has_pseudostates() {
        let layout = state_basic_layout();
        let json = render(&layout).expect("state render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();

        let initial = elements
            .iter()
            .find(|e| e["id"] == "initial")
            .expect("initial pseudostate present");
        assert_eq!(initial["type"], "ellipse");
        assert_eq!(initial["backgroundColor"], "#1e1e1e");
        assert_eq!(initial["fillStyle"], "solid");

        let outer = elements
            .iter()
            .find(|e| e["id"] == "final-outer")
            .expect("final-outer present");
        assert_eq!(outer["backgroundColor"], "transparent");

        let inner = elements
            .iter()
            .find(|e| e["id"] == "final-inner")
            .expect("final-inner present");
        assert_eq!(inner["backgroundColor"], "#1e1e1e");
        assert_eq!(inner["fillStyle"], "solid");

        // Transitions target the outer ring, never the decorative inner dot.
        assert!(!json.contains("\"elementId\": \"final-inner\""));
    }

    #[test]
    fn state_render_node_label_is_display_label() {
        let layout = state_basic_layout();
        let json = render(&layout).expect("state render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();
        let text = elements
            .iter()
            .find(|e| e["id"] == "n0-text")
            .expect("n0-text present");
        assert_eq!(text["text"], "Idle");
    }

    // --- sequence rendering ---

    #[test]
    fn sequence_render_has_lifeline_and_header() {
        let layout = seq_layout();
        let json = render(&layout).expect("sequence render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();

        let header = elements
            .iter()
            .find(|e| e["id"] == "n0")
            .expect("n0 header rectangle");
        assert_eq!(header["type"], "rectangle");

        let lifeline = elements
            .iter()
            .find(|e| e["id"] == "n0-lifeline")
            .expect("n0-lifeline present");
        assert_eq!(lifeline["type"], "line");
        assert_eq!(lifeline["strokeStyle"], "dashed");

        let text = elements
            .iter()
            .find(|e| e["id"] == "n0-text")
            .expect("n0-text present");
        assert_eq!(text["text"], "Alice");
    }

    #[test]
    fn sequence_render_message_arrow_has_no_bindings() {
        let layout = seq_layout();
        let json = render(&layout).expect("sequence render");
        let v = parse(&json);
        let arrow = v["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "e0")
            .expect("e0 message arrow present");
        assert!(arrow["startBinding"].is_null());
        assert!(arrow["endBinding"].is_null());
        let points = arrow["points"].as_array().unwrap();
        assert_eq!(points[0], serde_json::json!([0.0, 0.0]));
    }

    // --- error paths ---

    #[test]
    fn dangling_graph_edge_is_an_error() {
        // The layout engine auto-declares any node referenced by an edge, so
        // a genuinely dangling edge can only occur in a layout that is
        // mutated after the fact (e.g. by a future frontend/optimizer bug).
        // Layout a normal two-node graph, then point the edge's target at an
        // id that is not in `nodes` (all fields involved are `pub`, so this
        // is legal even though the enclosing structs are `#[non_exhaustive]`).
        let layout = graph_two_node_layout();
        let SemanticLayout::Graph(mut gl) = layout else {
            panic!("expected graph layout");
        };
        gl.edges[0].to.id = "does-not-exist".into();
        let err = render(&SemanticLayout::Graph(gl)).unwrap_err();
        assert_eq!(
            err,
            RenderError::DanglingEdge {
                node_id: "does-not-exist".to_string()
            }
        );
    }

    #[test]
    fn dangling_sequence_message_is_an_error() {
        let layout = seq_layout();
        let SemanticLayout::Sequence(mut sl) = layout else {
            panic!("expected sequence layout");
        };
        sl.messages[0].to = "does-not-exist".into();
        let err = render(&SemanticLayout::Sequence(sl)).unwrap_err();
        assert_eq!(
            err,
            RenderError::DanglingEdge {
                node_id: "does-not-exist".to_string()
            }
        );
    }

    #[test]
    fn render_error_display_mentions_kind() {
        let e = RenderError::UnsupportedDiagram { kind: "sequence" };
        assert!(e.to_string().contains("sequence"));
    }

    #[test]
    fn dangling_edge_error_display_mentions_node_id() {
        let e = RenderError::DanglingEdge {
            node_id: "missing_node".to_string(),
        };
        assert!(e.to_string().contains("missing_node"));
    }

    #[test]
    fn unknown_endpoint_error_display_mentions_description() {
        let e = RenderError::UnknownEndpoint {
            description: "Initial pseudostate not present".to_string(),
        };
        assert!(e.to_string().contains("Initial"));
    }

    // --- route geometry helper ---

    #[test]
    fn route_geometry_first_point_is_origin() {
        let route = vec![Point::new(5.0, 10.0), Point::new(15.0, 10.0)];
        let (x, y, points, w, h) = route_geometry(&route);
        assert_eq!(x, 5.0 + MARGIN);
        assert_eq!(y, 10.0 + MARGIN);
        assert_eq!(points[0], [0.0, 0.0]);
        assert_eq!(points[1], [10.0, 0.0]);
        assert_eq!(w, 10.0);
        assert_eq!(h, 0.0);
    }

    #[test]
    fn route_geometry_folded_route_bbox_encloses_fold() {
        // A route that folds outward and back (like a self-loop) must produce
        // a width/height that encloses the fold, not just the endpoint span.
        let route = vec![
            Point::new(0.0, 0.0),
            Point::new(20.0, 0.0),
            Point::new(20.0, -10.0),
            Point::new(0.0, -10.0),
        ];
        let (_, _, _, w, h) = route_geometry(&route);
        assert_eq!(w, 20.0);
        assert_eq!(h, 10.0);
    }

    // --- class rendering ---

    #[test]
    fn class_render_has_boxes_dividers_and_texts() {
        let layout = class_layout();
        let json = render(&layout).expect("class render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();

        let rect = elements
            .iter()
            .find(|e| e["id"] == "n0")
            .expect("n0 rectangle present");
        assert_eq!(rect["type"], "rectangle");

        assert!(
            elements.iter().any(|e| e["type"] == "line"),
            "compartment divider lines must be present: {json}"
        );

        let title = elements
            .iter()
            .find(|e| e["id"] == "n0-title")
            .expect("title text present");
        assert!(title["text"].as_str().unwrap().contains("Animal"));

        assert!(
            elements.iter().any(|e| e["text"]
                .as_str()
                .is_some_and(|t| t.contains("+speak(): void"))),
            "method row text must appear: {json}"
        );
    }

    #[test]
    fn class_render_arrowheads_are_approximated() {
        let layout = class_layout();
        let json = render(&layout).expect("class render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();

        let inherit = elements
            .iter()
            .find(|e| e["id"] == "e0")
            .expect("inheritance arrow");
        assert_eq!(inherit["endArrowhead"], "triangle_outline");

        let compose = elements
            .iter()
            .find(|e| e["id"] == "e1")
            .expect("composition arrow");
        assert_eq!(compose["startArrowhead"], "diamond");
        assert_eq!(compose["strokeStyle"], "dashed");
    }

    #[test]
    fn class_render_multiplicities_are_free_texts() {
        let layout = class_layout();
        let json = render(&layout).expect("class render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();
        assert!(elements.iter().any(|e| e["text"] == "1"));
        assert!(elements.iter().any(|e| e["text"] == "*"));
    }

    #[test]
    fn class_render_is_deterministic() {
        let layout = class_layout();
        let json1 = render(&layout).expect("render 1");
        let json2 = render(&layout).expect("render 2");
        assert_eq!(json1, json2, "class render must be deterministic");
    }

    // --- er rendering ---

    #[test]
    fn er_render_has_entity_rows_and_crowsfoot_approximation() {
        let layout = er_layout();
        let json = render(&layout).expect("er render");
        let v = parse(&json);
        let elements = v["elements"].as_array().unwrap();

        assert!(
            elements.iter().any(|e| e["text"]
                .as_str()
                .is_some_and(|t| t.contains("customer_id"))),
            "attribute row text must appear: {json}"
        );

        let rel = elements
            .iter()
            .find(|e| e["id"] == "e0")
            .expect("relation arrow");
        assert_eq!(rel["startArrowhead"], "bar");
        assert_eq!(rel["endArrowhead"], "dot");
    }

    #[test]
    fn er_render_is_deterministic() {
        let layout = er_layout();
        let json1 = render(&layout).expect("render 1");
        let json2 = render(&layout).expect("render 2");
        assert_eq!(json1, json2, "er render must be deterministic");
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
}
