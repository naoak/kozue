//! Sugiyama-style layered layout (M1).
//!
//! Pipeline:
//! 1. Cycle removal ([`cycle`]): DFS back edges are reversed for layout only;
//!    arrows are drawn in the original direction.
//! 2. Layer assignment ([`layering`]): longest-path method on the DAG.
//! 3. Dummy insertion + crossing reduction ([`ordering`]): long edges are
//!    split with dummy nodes; barycenter sweeps reduce crossings.
//! 4. Coordinate assignment ([`coords`]): mean-neighbor heuristic with exact
//!    overlap resolution (pool-adjacent-violators).
//!
//! Edges are routed as polylines through their dummy nodes. The layout also
//! owns the scene bounds ([`bounds`]): items are normalized so the top-left
//! corner is the origin and `Scene.width`/`Scene.height` cover everything,
//! including text, edge labels and arrowheads.

mod bounds;
mod boxes;
mod class;
mod coords;
mod cycle;
mod er;
mod layering;
mod markers;
mod ordering;
pub mod semantic;
mod sequence;
mod state;

use indexmap::IndexMap;
use kozue_ir::{
    ArrowType, Diagram, Direction, ElementId, GraphDiagram, Path, Rect, Scene, SceneItem, Text,
    TextAlign,
};

pub use semantic::SemanticLayout;

pub(crate) const FONT_SIZE: f64 = 16.0;
pub(crate) const PAD_X: f64 = 20.0;
pub(crate) const PAD_Y: f64 = 10.0;
pub(crate) const NODE_GAP: f64 = 40.0; // minimum clearance between nodes within a layer
pub(crate) const LAYER_GAP_DOWN: f64 = 100.0;
pub(crate) const LAYER_GAP_RIGHT: f64 = 150.0;
pub(crate) const ARROW_LEN: f64 = 10.0;
pub(crate) const ARROW_HALF_W: f64 = 5.0;

/// An error produced by the layout pass.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutError {
    pub message: String,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LayoutError {}

/// A positioned node box.
pub(crate) struct Placed {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) label: String,
}

impl Placed {
    pub(crate) fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// The combined output of a full layout pass: the renderable scene together with
/// the semantic-to-geometry mapping.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutOutput {
    /// The renderable scene (drawing primitives). Identical to the value
    /// returned by [`layout`].
    pub scene: Scene,
    /// Semantic-to-geometry mapping for exchange exporters, editors, etc.
    pub semantic: SemanticLayout,
}

/// Lay out a semantic [`Diagram`] into a [`Scene`] together with the
/// [`SemanticLayout`] that maps diagram elements to their geometric positions.
///
/// Cycles are supported: back edges are reversed internally for layering and
/// drawn in their original direction. Self-loop edges are rejected.
pub fn layout_full(diagram: &Diagram) -> Result<LayoutOutput, LayoutError> {
    match diagram {
        Diagram::Graph(g) => layout_graph_full(g),
        Diagram::Sequence(s) => Ok(sequence::layout_sequence_full(s)),
        Diagram::State(s) => state::layout_state_full(s),
        Diagram::Class(c) => class::layout_class_full(c),
        Diagram::Er(e) => er::layout_er_full(e),
        _ => Err(LayoutError {
            message: "unsupported diagram variant".to_string(),
        }),
    }
}

/// Lay out a semantic [`Diagram`] into a [`Scene`].
///
/// Cycles are supported: back edges are reversed internally for layering and
/// drawn in their original direction. Self-loop edges are rejected.
///
/// This is a backward-compatible wrapper around [`layout_full`]; it returns
/// only the [`Scene`] and discards the [`SemanticLayout`].
pub fn layout(diagram: &Diagram) -> Result<Scene, LayoutError> {
    layout_full(diagram).map(|o| o.scene)
}

/// Pure-geometry result of computing one edge's route.
///
/// Produced by [`compute_edge_geom`] and consumed by both
/// [`push_edge_geom`] (which emits [`SceneItem`]s) and the [`SemanticLayout`]
/// builder. This ensures the Scene and the SemanticLayout are always derived
/// from the same clipped endpoint computation.
pub(crate) struct EdgeGeom {
    /// Routing points (clipped at node borders), source-to-target order.
    pub(crate) route: Vec<(f64, f64)>,
}

/// Compute the geometry for a single edge without emitting any [`SceneItem`]s.
///
/// `pts` are the routing points **in original edge orientation** (from center,
/// bends…, to center). The function clips the endpoints to the node borders
/// and returns the full route.
///
/// Label anchor computation is left to the caller because it requires the
/// actual label text (for width-aware displacement of mutual-edge labels).
pub(crate) fn compute_edge_geom(mut pts: Vec<(f64, f64)>, from: &Placed, to: &Placed) -> EdgeGeom {
    let last = pts.len() - 1;
    pts[0] = clip_to_rect(from, pts[1].0, pts[1].1);
    let end = clip_to_rect(to, pts[last - 1].0, pts[last - 1].1);
    pts[last] = end;

    EdgeGeom { route: pts }
}

/// Compute the anchor (text center) for an edge/transition label.
///
/// Single source of truth shared by [`push_edge_geom`] (Scene text position)
/// and the `SemanticLayout` builders (graph/state), so the two never diverge.
///
/// - `route` is the clipped polyline; the anchor sits at its arc-length midpoint.
/// - For an ordinary single edge (`offset == (0, 0)`) the anchor is the midpoint
///   lifted 4px up.
/// - For a mutual edge (non-zero `offset`) the anchor is displaced along the
///   offset direction by half the label's projected extent plus [`LABEL_GAP`],
///   so the two mutual labels never overlap regardless of text length.
pub(crate) fn edge_label_anchor(
    route: &[(f64, f64)],
    tw: f64,
    th: f64,
    offset: (f64, f64),
) -> (f64, f64) {
    let (mx, my) = polyline_midpoint(route);
    if offset == (0.0, 0.0) {
        (mx, my - 4.0)
    } else {
        let (ox, oy) = offset;
        let len = (ox * ox + oy * oy).sqrt().max(1e-6);
        let (ux, uy) = (ox / len, oy / len);
        let half = ux.abs() * (tw / 2.0) + uy.abs() * (th / 2.0);
        let d = half + LABEL_GAP;
        (mx + ux * d, my + uy * d)
    }
}

/// Emit [`SceneItem`]s for one edge given its pre-computed geometry.
///
/// This is the "emit" half of what was previously a single `push_edge`
/// function. It produces identical output to the old `push_edge`.
pub(crate) fn push_edge_geom(
    items: &mut Vec<SceneItem>,
    geom: &EdgeGeom,
    label: Option<&str>,
    arrow: ArrowType,
    label_offset: (f64, f64),
) {
    let pts = &geom.route;
    let last = pts.len() - 1;
    let end = pts[last];

    let draw_arrow = !matches!(arrow, ArrowType::None);

    if draw_arrow {
        let dx = end.0 - pts[last - 1].0;
        let dy = end.1 - pts[last - 1].1;
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let ux = dx / len;
        let uy = dy / len;
        let line_end = (end.0 - ux * ARROW_LEN, end.1 - uy * ARROW_LEN);

        let mut line_pts = pts.clone();
        line_pts[last] = line_end;
        items.push(SceneItem::Path(Path {
            points: line_pts,
            filled: false,
            dashed: false,
        }));

        let px = -uy;
        let py = ux;
        let left = (
            line_end.0 + px * ARROW_HALF_W,
            line_end.1 + py * ARROW_HALF_W,
        );
        let right = (
            line_end.0 - px * ARROW_HALF_W,
            line_end.1 - py * ARROW_HALF_W,
        );
        items.push(SceneItem::Path(Path {
            points: vec![end, left, right],
            filled: true,
            dashed: false,
        }));
    } else {
        items.push(SceneItem::Path(Path {
            points: pts.clone(),
            filled: false,
            dashed: false,
        }));
    }

    if let Some(label) = label {
        let (tw, th) = kozue_text::measure(label, FONT_SIZE * 0.85);
        let (lx, ly) = edge_label_anchor(pts, tw, th, label_offset);
        items.push(SceneItem::Text(Text {
            x: lx,
            y: ly,
            size: FONT_SIZE * 0.85,
            align: TextAlign::Middle,
            content: label.to_string(),
            text_width: tw,
            text_height: th,
        }));
    }
}

fn layout_graph_full(g: &GraphDiagram) -> Result<LayoutOutput, LayoutError> {
    // Node index order = declaration order.
    let ids: Vec<&ElementId> = g.nodes.keys().collect();
    let index_of: IndexMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let n = ids.len();

    // Resolve edge endpoints (skipping edges with unknown endpoints, which
    // the DSL already rejects).
    let mut raw_edges: Vec<(usize, usize)> = Vec::new();
    let mut edge_ids: Vec<usize> = Vec::new();
    for (i, e) in g.edges.iter().enumerate() {
        let (Some(&from), Some(&to)) = (index_of.get(e.from.as_str()), index_of.get(e.to.as_str()))
        else {
            continue;
        };
        if from == to {
            return Err(LayoutError {
                message: format!("self-loop edges are not supported ({} -> {})", e.from, e.to),
            });
        }
        raw_edges.push((from, to));
        edge_ids.push(i);
    }

    // Phase 1: cycle removal (layout-internal reversal of back edges).
    let reversed = cycle::greedy_reversed(n, &raw_edges);
    let acyclic: Vec<(usize, usize)> = raw_edges
        .iter()
        .zip(&reversed)
        .map(|(&(u, v), &r)| if r { (v, u) } else { (u, v) })
        .collect();

    // Phase 2: layer assignment (longest path).
    let layers = layering::longest_path(n, &acyclic);

    // Measure each node's box.
    let boxes: Vec<(f64, f64, String)> = ids
        .iter()
        .map(|id| {
            let node = &g.nodes[*id];
            let (tw, th) = kozue_text::measure(&node.label, FONT_SIZE);
            (tw + 2.0 * PAD_X, th + 2.0 * PAD_Y, node.label.clone())
        })
        .collect();

    // Map (width, height) onto (cross, main) axes per direction.
    let horizontal = g.direction == Direction::Right;
    let sizes: Vec<(f64, f64)> = boxes
        .iter()
        .map(|&(w, h, _)| if horizontal { (h, w) } else { (w, h) })
        .collect();

    // Phase 3: dummy insertion + barycenter crossing reduction.
    let mut lay = ordering::build(n, &sizes, &layers, &acyclic);
    ordering::reduce_crossings(&mut lay);

    // Phase 4: coordinate assignment.
    let cross = coords::assign_cross(&lay, NODE_GAP);

    // Main-axis positions per layer.
    let nl = lay.order.len();
    let layer_gap = if horizontal {
        LAYER_GAP_RIGHT
    } else {
        LAYER_GAP_DOWN
    };
    let mut layer_start = vec![0.0f64; nl];
    let mut layer_size = vec![0.0f64; nl];
    let mut cursor = 0.0f64;
    for l in 0..nl {
        let size = lay.order[l]
            .iter()
            .map(|&v| lay.main_size[v])
            .fold(0.0f64, f64::max);
        layer_start[l] = cursor;
        layer_size[l] = size;
        cursor += size + layer_gap;
    }

    // Place real nodes.
    let placed: Vec<Placed> = (0..n)
        .map(|v| {
            let (w, h, ref label) = boxes[v];
            let main = layer_start[lay.layer_of[v]];
            let (x, y) = if horizontal {
                (main, cross[v] - h / 2.0)
            } else {
                (cross[v] - w / 2.0, main)
            };
            Placed {
                x,
                y,
                width: w,
                height: h,
                label: label.clone(),
            }
        })
        .collect();

    // Routing point for any lnode: real node center or dummy point at the
    // middle of its layer band.
    let point_of = |v: usize| -> (f64, f64) {
        if lay.is_dummy[v] {
            let l = lay.layer_of[v];
            let main = layer_start[l] + layer_size[l] / 2.0;
            if horizontal {
                (main, cross[v])
            } else {
                (cross[v], main)
            }
        } else {
            placed[v].center()
        }
    };

    // Build scene items: nodes first, then edges.
    let mut items: Vec<SceneItem> = Vec::new();

    // Semantic node layouts (pre-translation; adjusted below).
    let mut sem_nodes: Vec<semantic::NodeLayout> = Vec::new();

    for (v, p) in placed.iter().enumerate() {
        items.push(SceneItem::Rect(Rect {
            x: p.x,
            y: p.y,
            width: p.width,
            height: p.height,
            rx: 4.0,
        }));
        let (cx, cy) = p.center();
        let (tw, th) = kozue_text::measure(&p.label, FONT_SIZE);
        items.push(SceneItem::Text(Text {
            x: cx,
            y: cy + FONT_SIZE * 0.35, // rough baseline centering
            size: FONT_SIZE,
            align: TextAlign::Middle,
            content: p.label.clone(),
            text_width: tw,
            text_height: th,
        }));

        sem_nodes.push(semantic::NodeLayout {
            id: ids[v].clone(),
            label: p.label.clone(),
            rect: Rect {
                x: p.x,
                y: p.y,
                width: p.width,
                height: p.height,
                rx: 4.0,
            },
            label_anchor: semantic::Point::new(cx, cy + FONT_SIZE * 0.35),
        });
    }

    // Separate parallel/mutual edges so they don't draw on top of each other.
    let offsets = parallel_edge_offsets(&raw_edges, &placed);
    let mut sem_edges: Vec<semantic::EdgeLayout> = Vec::new();

    for (k, &(from, to)) in raw_edges.iter().enumerate() {
        // Chain points in layout (acyclic) orientation; restore the original
        // direction so the arrowhead points along the declared edge.
        let mut pts: Vec<(f64, f64)> = lay.chains[k].iter().map(|&v| point_of(v)).collect();
        if reversed[k] {
            pts.reverse();
        }
        bow_polyline(&mut pts, offsets[k]);
        let edge = &g.edges[edge_ids[k]];

        // Compute geometry once; emit scene items and build SemanticLayout from it.
        let geom = compute_edge_geom(pts, &placed[from], &placed[to]);
        // Label anchor from the shared helper (same value as the Scene text).
        let sem_label_anchor = edge.label.as_deref().map(|lbl| {
            let (tw, th) = kozue_text::measure(lbl, FONT_SIZE * 0.85);
            let (lx, ly) = edge_label_anchor(&geom.route, tw, th, offsets[k]);
            semantic::Point::new(lx, ly)
        });

        push_edge_geom(
            &mut items,
            &geom,
            edge.label.as_deref(),
            edge.arrow,
            offsets[k],
        );

        sem_edges.push(semantic::EdgeLayout {
            index: edge_ids[k],
            from: semantic::GraphEndpoint::new(g.edges[edge_ids[k]].from.clone()),
            to: semantic::GraphEndpoint::new(g.edges[edge_ids[k]].to.clone()),
            arrow: edge.arrow,
            route: geom
                .route
                .iter()
                .map(|&(x, y)| semantic::Point::new(x, y))
                .collect(),
            label: edge.label.clone(),
            label_anchor: sem_label_anchor,
        });
    }

    // Normalize: layout owns the bounds, including text and arrowheads.
    let (min_x, min_y, max_x, max_y) = bounds::scene_bounds(&items);
    bounds::translate(&mut items, -min_x, -min_y);

    // Apply the same translation to all semantic coordinates.
    for nl in &mut sem_nodes {
        nl.rect.x -= min_x;
        nl.rect.y -= min_y;
        nl.label_anchor.x -= min_x;
        nl.label_anchor.y -= min_y;
    }
    for el in &mut sem_edges {
        for pt in &mut el.route {
            pt.x -= min_x;
            pt.y -= min_y;
        }
        if let Some(la) = &mut el.label_anchor {
            la.x -= min_x;
            la.y -= min_y;
        }
    }

    let scene = Scene {
        width: max_x - min_x,
        height: max_y - min_y,
        items,
    };
    let semantic = SemanticLayout::Graph(semantic::GraphLayout {
        nodes: sem_nodes,
        edges: sem_edges,
    });

    Ok(LayoutOutput { scene, semantic })
}

/// Draw one edge as a polyline through its bend points, with an optional
/// arrowhead at the target end and an optional label at the polyline midpoint.
///
/// `pts` are the edge's routing points in original edge orientation:
/// `[from_center, bends.., to_center]` (length >= 2).
/// Perpendicular spacing between parallel edges sharing the same node pair.
pub(crate) const EDGE_SEP: f64 = 14.0;
/// Clearance between a mutual-edge label and the shared midpoint.
pub(crate) const LABEL_GAP: f64 = 4.0;

/// Compute a lateral offset vector for each edge so that multiple edges between
/// the same pair of nodes (e.g. mutual `a -> b` / `b -> a`) are drawn apart
/// instead of coincident. Edges with no parallel sibling get `(0.0, 0.0)`, so
/// single-edge layouts are unchanged.
///
/// Edges are grouped by their *unordered* endpoint pair; the offset is measured
/// along the perpendicular of that pair's canonical (low→high index) direction,
/// so the two directions of a mutual pair land on opposite sides. Deterministic:
/// grouping uses an `IndexMap` and offsets derive only from group membership.
pub(crate) fn parallel_edge_offsets(
    raw_edges: &[(usize, usize)],
    placed: &[Placed],
) -> Vec<(f64, f64)> {
    let mut groups: IndexMap<(usize, usize), Vec<usize>> = IndexMap::new();
    for (k, &(u, v)) in raw_edges.iter().enumerate() {
        groups.entry((u.min(v), u.max(v))).or_default().push(k);
    }

    let mut offsets = vec![(0.0, 0.0); raw_edges.len()];
    for (&(lo, hi), members) in &groups {
        let m = members.len();
        if m < 2 {
            continue;
        }
        // Perpendicular unit vector of the canonical lo→hi direction.
        let (lx, ly) = placed[lo].center();
        let (hx, hy) = placed[hi].center();
        let (dx, dy) = (hx - lx, hy - ly);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let (px, py) = (-dy / len, dx / len);
        for (i, &k) in members.iter().enumerate() {
            // Center the group around 0: e.g. two edges → -0.5, +0.5.
            let t = i as f64 - (m as f64 - 1.0) / 2.0;
            offsets[k] = (px * t * EDGE_SEP, py * t * EDGE_SEP);
        }
    }
    offsets
}

/// Apply a parallel-edge separation `offset` to a polyline. A straight two-point
/// edge gains a bowed midpoint (offset applied at the middle) so the separation
/// stays visible after the endpoints are clipped to the node borders; an already
/// bent (dummy-routed) edge is translated wholesale. No-op for a zero offset.
pub(crate) fn bow_polyline(pts: &mut Vec<(f64, f64)>, offset: (f64, f64)) {
    let (ox, oy) = offset;
    if ox == 0.0 && oy == 0.0 {
        return;
    }
    if pts.len() == 2 {
        let mid = (
            (pts[0].0 + pts[1].0) / 2.0 + ox,
            (pts[0].1 + pts[1].1) / 2.0 + oy,
        );
        pts.insert(1, mid);
    } else {
        for p in pts.iter_mut() {
            p.0 += ox;
            p.1 += oy;
        }
    }
}

/// The point at half the arc length of a polyline.
pub(crate) fn polyline_midpoint(pts: &[(f64, f64)]) -> (f64, f64) {
    let total: f64 = pts
        .windows(2)
        .map(|w| {
            let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
            (dx * dx + dy * dy).sqrt()
        })
        .sum();
    if total < 1e-9 {
        return pts[0];
    }
    let mut remaining = total / 2.0;
    for w in pts.windows(2) {
        let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
        let seg = (dx * dx + dy * dy).sqrt();
        if seg >= remaining {
            let t = remaining / seg.max(1e-9);
            return (w[0].0 + dx * t, w[0].1 + dy * t);
        }
        remaining -= seg;
    }
    *pts.last().unwrap()
}

const CIRCLE_POINTS: usize = 20;

/// A regular polygon approximating a circle, as a [`Path`]. Shared by state
/// pseudostates ([`state`]) and ER "zero" crow's-foot markers ([`markers`]).
pub(crate) fn circle_path(cx: f64, cy: f64, r: f64, filled: bool) -> Path {
    let mut points: Vec<(f64, f64)> = (0..CIRCLE_POINTS)
        .map(|i| {
            let angle = i as f64 * 2.0 * std::f64::consts::PI / CIRCLE_POINTS as f64;
            (cx + r * angle.cos(), cy + r * angle.sin())
        })
        .collect();
    // Close the ring: an unfilled circle renders as an open polyline, so repeat
    // the first point to join the last segment back to the start (otherwise the
    // stroked outer circle has a visible gap at angle 0).
    if let Some(&first) = points.first() {
        points.push(first);
    }
    Path {
        points,
        filled,
        dashed: false,
    }
}

/// Return the point on the rectangle border of `p` along the ray from the
/// center toward `(tx, ty)`.
fn clip_to_rect(p: &Placed, tx: f64, ty: f64) -> (f64, f64) {
    let (cx, cy) = p.center();
    let dx = tx - cx;
    let dy = ty - cy;
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return (cx, cy);
    }
    let hw = p.width / 2.0;
    let hh = p.height / 2.0;
    // Scale factor to hit each pair of borders.
    let sx = if dx.abs() > 1e-9 {
        hw / dx.abs()
    } else {
        f64::INFINITY
    };
    let sy = if dy.abs() > 1e-9 {
        hh / dy.abs()
    } else {
        f64::INFINITY
    };
    let s = sx.min(sy);
    (cx + dx * s, cy + dy * s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kozue_ir::{ArrowType, Edge, Node};

    fn node(id: &str, label: &str) -> Node {
        Node::new(id, label)
    }

    fn edge(from: &str, to: &str) -> Edge {
        Edge::new(from, to, None, ArrowType::Triangle)
    }

    /// Node rectangles in declaration order (edge paths are not rects).
    fn rects(scene: &Scene) -> Vec<&Rect> {
        scene
            .items
            .iter()
            .filter_map(|i| match i {
                SceneItem::Rect(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    fn open_paths(scene: &Scene) -> Vec<&Path> {
        scene
            .items
            .iter()
            .filter_map(|i| match i {
                SceneItem::Path(p) if !p.filled => Some(p),
                _ => None,
            })
            .collect()
    }

    fn filled_paths(scene: &Scene) -> Vec<&Path> {
        scene
            .items
            .iter()
            .filter_map(|i| match i {
                SceneItem::Path(p) if p.filled => Some(p),
                _ => None,
            })
            .collect()
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.x < b.x + b.width - 1e-6
            && b.x < a.x + a.width - 1e-6
            && a.y < b.y + b.height - 1e-6
            && b.y < a.y + a.height - 1e-6
    }

    #[test]
    fn scene_has_positive_bounds() {
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "A"));
        g.nodes.insert("b".into(), node("b", "B"));
        g.edges.push(edge("a", "b"));
        let scene = layout(&Diagram::Graph(g)).expect("layout");
        assert!(scene.width > 0.0);
        assert!(scene.height > 0.0);
    }

    #[test]
    fn parallel_edge_offsets_only_separates_mutual_pairs() {
        // A single a->b edge and one mutual pair. The singleton gets a zero
        // offset (goldens unchanged); the mutual pair gets equal-and-opposite
        // non-zero offsets.
        let placed = vec![
            Placed {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 20.0,
                label: "a".into(),
            },
            Placed {
                x: 0.0,
                y: 100.0,
                width: 40.0,
                height: 20.0,
                label: "b".into(),
            },
        ];
        // Singleton.
        assert_eq!(parallel_edge_offsets(&[(0, 1)], &placed), vec![(0.0, 0.0)]);
        // Mutual pair a->b, b->a.
        let offs = parallel_edge_offsets(&[(0, 1), (1, 0)], &placed);
        assert!(offs[0] != (0.0, 0.0) && offs[1] != (0.0, 0.0));
        assert!(
            (offs[0].0 + offs[1].0).abs() < 1e-9 && (offs[0].1 + offs[1].1).abs() < 1e-9,
            "offsets must be equal and opposite, got {offs:?}"
        );
    }

    #[test]
    fn mutual_edge_labels_never_overlap_even_when_long() {
        // Root-cause regression: mutual-edge labels must not overlap regardless
        // of text length. Displacement scales with each label's own width, so a
        // long label extends further outward instead of crossing the midline.
        for (la, lb) in [
            ("go", "back"),
            (
                "a very long transition label",
                "another extremely long label here",
            ),
        ] {
            let mut g = GraphDiagram::new(Direction::Down);
            g.nodes.insert("a".into(), node("a", "A"));
            g.nodes.insert("b".into(), node("b", "B"));
            let mut e1 = edge("a", "b");
            e1.label = Some(la.to_string());
            let mut e2 = edge("b", "a");
            e2.label = Some(lb.to_string());
            g.edges.push(e1);
            g.edges.push(e2);
            let scene = layout(&Diagram::Graph(g)).expect("layout");

            let labels: Vec<&Text> = scene
                .items
                .iter()
                .filter_map(|i| match i {
                    SceneItem::Text(t) if t.content == la || t.content == lb => Some(t),
                    _ => None,
                })
                .collect();
            assert_eq!(labels.len(), 2, "both labels present");
            // Middle-anchored: horizontal span is x ± text_width/2. The two must
            // not overlap in x (edges here are vertical → labels split L/R).
            let (l0, r0) = (
                labels[0].x - labels[0].text_width / 2.0,
                labels[0].x + labels[0].text_width / 2.0,
            );
            let (l1, r1) = (
                labels[1].x - labels[1].text_width / 2.0,
                labels[1].x + labels[1].text_width / 2.0,
            );
            assert!(
                r0 <= l1 || r1 <= l0,
                "labels {la:?}/{lb:?} overlap: [{l0},{r0}] vs [{l1},{r1}]"
            );
        }
    }

    #[test]
    fn mutual_graph_edges_are_not_coincident() {
        // Regression (M1 carryover): a <-> b must render as two separated lines.
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "A"));
        g.nodes.insert("b".into(), node("b", "B"));
        g.edges.push(edge("a", "b"));
        g.edges.push(edge("b", "a"));
        let scene = layout(&Diagram::Graph(g)).expect("layout");
        // The two edge polylines must not be identical.
        let lines: Vec<&Path> = open_paths(&scene);
        assert!(lines.len() >= 2);
        assert_ne!(
            lines[0].points, lines[1].points,
            "mutual edges must be separated, not coincident"
        );
    }

    /// direction=down で同一層に複数ノードが並ぶ場合、cross軸=X方向に
    /// 幅の和+GAP 以上の広がりを持つこと。
    #[test]
    fn cross_extent_down_multi_node_layer() {
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "Alpha"));
        g.nodes.insert("b".into(), node("b", "Beta"));
        // No edges → both in layer 0.
        let scene = layout(&Diagram::Graph(g)).expect("layout");
        let (single_w, _) = kozue_text::measure("Alpha", FONT_SIZE);
        let single_box_w = single_w + 2.0 * PAD_X;
        assert!(
            scene.width > single_box_w,
            "scene width {} should exceed single node box width {}",
            scene.width,
            single_box_w
        );
    }

    /// direction=right で同一層に複数ノードが並ぶ場合、cross軸=Y方向に
    /// 高さの和+GAP 以上の広がりを持つこと (M0の軸バグ回帰防止)。
    #[test]
    fn cross_extent_right_multi_node_layer() {
        let mut g = GraphDiagram::new(Direction::Right);
        g.nodes.insert("a".into(), node("a", "Alpha"));
        g.nodes.insert("b".into(), node("b", "Beta"));
        // No edges → both in layer 0.
        let scene = layout(&Diagram::Graph(g)).expect("layout");
        let (_, single_h) = kozue_text::measure("Alpha", FONT_SIZE);
        let single_box_h = single_h + 2.0 * PAD_Y;
        assert!(
            scene.height > single_box_h,
            "scene height {} should exceed single node box height {}",
            scene.height,
            single_box_h
        );
    }

    /// サイクルはレイアウト内部で一時反転され、矢印は元の向きで描かれる。
    #[test]
    fn two_node_cycle_keeps_original_arrow_directions() {
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "A"));
        g.nodes.insert("b".into(), node("b", "B"));
        g.edges.push(edge("a", "b"));
        g.edges.push(edge("b", "a"));
        let scene = layout(&Diagram::Graph(g)).expect("cycles must be supported");

        let rects = rects(&scene);
        assert_eq!(rects.len(), 2);
        let (ra, rb) = (rects[0], rects[1]); // declaration order: a, b
        assert!(ra.y < rb.y, "a should be in the upper layer");

        // Arrowhead tips (first point of each filled triangle), in edge order.
        let arrows = filled_paths(&scene);
        assert_eq!(arrows.len(), 2);
        let tip_ab = arrows[0].points[0];
        let tip_ba = arrows[1].points[0];
        // a -> b: tip on b's top border. b -> a: tip on a's bottom border.
        assert!(
            (tip_ab.1 - rb.y).abs() < 1e-6,
            "a->b arrow must point into b (tip y {} vs b top {})",
            tip_ab.1,
            rb.y
        );
        assert!(
            (tip_ba.1 - (ra.y + ra.height)).abs() < 1e-6,
            "b->a arrow must point back into a (tip y {} vs a bottom {})",
            tip_ba.1,
            ra.y + ra.height
        );
    }

    #[test]
    fn three_node_cycle_layouts() {
        let mut g = GraphDiagram::new(Direction::Down);
        for id in ["a", "b", "c"] {
            g.nodes.insert(id.into(), node(id, id));
        }
        g.edges.push(edge("a", "b"));
        g.edges.push(edge("b", "c"));
        g.edges.push(edge("c", "a"));
        let scene = layout(&Diagram::Graph(g)).expect("cycles must be supported");
        assert_eq!(rects(&scene).len(), 3);
        assert_eq!(filled_paths(&scene).len(), 3);
    }

    /// 3層以上またぐエッジはダミーノード経由の折れ線になる。
    #[test]
    fn long_edge_is_routed_as_polyline() {
        let mut g = GraphDiagram::new(Direction::Down);
        for id in ["a", "b", "c", "d"] {
            g.nodes.insert(id.into(), node(id, id));
        }
        g.edges.push(edge("a", "b"));
        g.edges.push(edge("b", "c"));
        g.edges.push(edge("c", "d"));
        g.edges.push(edge("a", "d"));
        let scene = layout(&Diagram::Graph(g)).expect("layout");
        let paths = open_paths(&scene);
        assert_eq!(paths.len(), 4);
        // Edge order matches declaration order; a->d spans 3 layers → 2 bends.
        assert_eq!(paths[3].points.len(), 4, "a->d must bend at two dummies");
        for p in &paths[0..3] {
            assert_eq!(p.points.len(), 2, "adjacent-layer edges stay straight");
        }
    }

    /// 直線チェーンはまっすぐ一列になる (direction down)。
    #[test]
    fn straight_chain_is_collinear_down() {
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "short"));
        g.nodes.insert("b".into(), node("b", "a much longer label"));
        g.nodes.insert("c".into(), node("c", "mid"));
        g.edges.push(edge("a", "b"));
        g.edges.push(edge("b", "c"));
        let scene = layout(&Diagram::Graph(g)).expect("layout");
        let rects = rects(&scene);
        let cx0 = rects[0].x + rects[0].width / 2.0;
        for r in &rects[1..] {
            let cx = r.x + r.width / 2.0;
            assert!((cx - cx0).abs() < 1e-6, "chain must be vertically aligned");
        }
    }

    /// 直線チェーンはまっすぐ一列になる (direction right)。
    #[test]
    fn straight_chain_is_collinear_right() {
        let mut g = GraphDiagram::new(Direction::Right);
        g.nodes.insert("a".into(), node("a", "short"));
        g.nodes.insert("b".into(), node("b", "a much longer label"));
        g.nodes.insert("c".into(), node("c", "mid"));
        g.edges.push(edge("a", "b"));
        g.edges.push(edge("b", "c"));
        let scene = layout(&Diagram::Graph(g)).expect("layout");
        let rects = rects(&scene);
        let cy0 = rects[0].y + rects[0].height / 2.0;
        for r in &rects[1..] {
            let cy = r.y + r.height / 2.0;
            assert!(
                (cy - cy0).abs() < 1e-6,
                "chain must be horizontally aligned"
            );
        }
    }

    /// 固定の複雑な図でノードbox同士が重ならないこと (両方向)。
    fn complex_graph(direction: Direction) -> GraphDiagram {
        let mut g = GraphDiagram::new(direction);
        for (id, label) in [
            ("a", "Entry point"),
            ("b", "Branch"),
            ("c", "Compute"),
            ("d", "Dispatch"),
            ("e", "Evaluate"),
            ("f", "Finish"),
            ("g", "Guard"),
            ("h", "Handle"),
        ] {
            g.nodes.insert(id.into(), node(id, label));
        }
        for (from, to) in [
            ("a", "b"),
            ("a", "c"),
            ("a", "d"),
            ("b", "e"),
            ("c", "f"),
            ("d", "e"),
            ("b", "f"),
            ("e", "g"),
            ("f", "g"),
            ("a", "g"), // long edge
            ("g", "h"),
            ("h", "b"), // cycle back
            ("d", "h"), // long edge
        ] {
            g.edges.push(edge(from, to));
        }
        g
    }

    #[test]
    fn complex_graph_has_no_node_overlap_down() {
        let scene = layout(&Diagram::Graph(complex_graph(Direction::Down))).expect("layout");
        let rects = rects(&scene);
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                assert!(
                    !overlaps(rects[i], rects[j]),
                    "nodes {i} and {j} overlap: {:?} vs {:?}",
                    (rects[i].x, rects[i].y, rects[i].width, rects[i].height),
                    (rects[j].x, rects[j].y, rects[j].width, rects[j].height),
                );
            }
        }
    }

    #[test]
    fn complex_graph_has_no_node_overlap_right() {
        let scene = layout(&Diagram::Graph(complex_graph(Direction::Right))).expect("layout");
        let rects = rects(&scene);
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                assert!(
                    !overlaps(rects[i], rects[j]),
                    "nodes {i} and {j} overlap in direction=right"
                );
            }
        }
    }

    /// Scene.width/height はテキスト・ラベル・矢印を含む正規化済み境界。
    #[test]
    fn bounds_are_normalized_and_cover_everything() {
        let mut g = GraphDiagram::new(Direction::Right);
        g.nodes.insert("a".into(), node("a", "A"));
        g.nodes.insert("b".into(), node("b", "B"));
        g.edges.push(Edge::new(
            "a",
            "b",
            Some("a very long edge label indeed".to_string()),
            ArrowType::Triangle,
        ));
        let scene = layout(&Diagram::Graph(g)).expect("layout");
        let (min_x, min_y, max_x, max_y) = bounds::scene_bounds(&scene.items);
        assert!(
            min_x.abs() < 1e-9 && min_y.abs() < 1e-9,
            "origin normalized"
        );
        assert!((max_x - scene.width).abs() < 1e-9);
        assert!((max_y - scene.height).abs() < 1e-9);
        // The long edge label must widen the scene beyond the node boxes.
        let rects = rects(&scene);
        let node_max_y = rects.iter().map(|r| r.y + r.height).fold(0.0f64, f64::max);
        assert!(scene.height >= node_max_y);
    }

    #[test]
    fn self_loop_is_error() {
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "A"));
        g.edges.push(edge("a", "a"));
        let result = layout(&Diagram::Graph(g));
        assert!(result.is_err(), "self loops must be rejected");
    }

    // --- Sequence diagram layout tests ---

    fn seq_participant(seq: &mut kozue_ir::SequenceDiagram, id: &str, label: &str) {
        seq.participants
            .insert(id.into(), kozue_ir::Participant::new(id, label));
    }

    fn seq_message(seq: &mut kozue_ir::SequenceDiagram, from: &str, to: &str, label: Option<&str>) {
        seq.items
            .push(kozue_ir::SequenceItem::Message(kozue_ir::Message::new(
                from,
                to,
                label.map(str::to_string),
                kozue_ir::LineStyle::Solid,
                kozue_ir::ArrowType::Triangle,
            )));
    }

    /// Issue 4b: Single participant with 0 messages must not panic and must
    /// produce a valid Scene (positive or zero dimensions).
    #[test]
    fn single_participant_no_messages_does_not_panic() {
        let mut seq = kozue_ir::SequenceDiagram::new();
        seq_participant(&mut seq, "solo", "Solo");
        let scene = layout(&Diagram::Sequence(seq)).expect("layout must not fail");
        // Scene must have some positive dimensions (at least the header box).
        assert!(
            scene.width > 0.0 && scene.height > 0.0,
            "scene must have positive bounds: {}x{}",
            scene.width,
            scene.height
        );
    }

    /// Issue 4a: A long label on a -> c (2-gap span) must push out the middle
    /// column b. This exercises the fixup loop in col_x computation.
    #[test]
    fn long_spanning_label_pushes_middle_column() {
        // Build a narrow version (short label a -> c).
        let mut seq_narrow = kozue_ir::SequenceDiagram::new();
        seq_participant(&mut seq_narrow, "a", "A");
        seq_participant(&mut seq_narrow, "b", "B");
        seq_participant(&mut seq_narrow, "c", "C");
        seq_message(&mut seq_narrow, "a", "c", Some("hi"));
        let scene_narrow = layout(&Diagram::Sequence(seq_narrow)).expect("narrow layout");

        // Build a wide version (very long label a -> c).
        let mut seq_wide = kozue_ir::SequenceDiagram::new();
        seq_participant(&mut seq_wide, "a", "A");
        seq_participant(&mut seq_wide, "b", "B");
        seq_participant(&mut seq_wide, "c", "C");
        seq_message(
            &mut seq_wide,
            "a",
            "c",
            Some("this is a very very very long spanning message label"),
        );
        let scene_wide = layout(&Diagram::Sequence(seq_wide)).expect("wide layout");

        assert!(
            scene_wide.width > scene_narrow.width,
            "long spanning label ({}px wide) should push out the scene width beyond narrow ({} vs {})",
            0.0,
            scene_wide.width,
            scene_narrow.width,
        );
    }

    /// Issue 3: A long self-message label on a middle column must push the next
    /// column's header to the right so they don't overlap.
    #[test]
    fn long_self_message_label_does_not_overlap_next_column_header() {
        // Three participants: a, b (has long self-message), c.
        // b is in the middle; the self-message label on b should force c rightward.
        let mut seq_long = kozue_ir::SequenceDiagram::new();
        seq_participant(&mut seq_long, "a", "A");
        seq_participant(&mut seq_long, "b", "B");
        seq_participant(&mut seq_long, "c", "C");
        // A very long self-message on b.
        seq_message(
            &mut seq_long,
            "b",
            "b",
            Some("an extremely long self message label that should push column c to the right"),
        );
        let scene_long = layout(&Diagram::Sequence(seq_long)).expect("layout with long self-msg");

        // Same diagram with a short self-message on b.
        let mut seq_short = kozue_ir::SequenceDiagram::new();
        seq_participant(&mut seq_short, "a", "A");
        seq_participant(&mut seq_short, "b", "B");
        seq_participant(&mut seq_short, "c", "C");
        seq_message(&mut seq_short, "b", "b", Some("x"));
        let scene_short =
            layout(&Diagram::Sequence(seq_short)).expect("layout with short self-msg");

        // The long self-message must widen the scene (c column pushed rightward).
        assert!(
            scene_long.width > scene_short.width,
            "long self-message (col b) must widen scene: {} vs {} (short)",
            scene_long.width,
            scene_short.width,
        );

        // Also verify that in the long case, c's header left edge is to the right
        // of b's header right edge (no overlap between adjacent header boxes).
        let header_rects_long: Vec<&Rect> = scene_long
            .items
            .iter()
            .filter_map(|i| match i {
                SceneItem::Rect(r) => Some(r),
                _ => None,
            })
            .take(3)
            .collect();
        assert_eq!(header_rects_long.len(), 3, "expected 3 header rects");
        let b_right = header_rects_long[1].x + header_rects_long[1].width;
        let c_left = header_rects_long[2].x;
        assert!(
            c_left >= b_right - 1e-6,
            "c's header left ({}) must not overlap b's header right ({})",
            c_left,
            b_right
        );
    }

    // --- M7a: State diagram layout tests ---

    #[test]
    fn state_layout_basic_scene_has_positive_bounds() {
        let mut sd = kozue_ir::StateDiagram::new();
        sd.states
            .insert("idle".into(), kozue_ir::State::new("idle", "Idle"));
        sd.states
            .insert("active".into(), kozue_ir::State::new("active", "Active"));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::Initial,
            kozue_ir::Endpoint::State("idle".into()),
            None,
        ));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::State("idle".into()),
            kozue_ir::Endpoint::State("active".into()),
            Some("start".to_string()),
        ));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::State("active".into()),
            kozue_ir::Endpoint::Final,
            None,
        ));
        let scene = layout(&Diagram::State(sd)).expect("state layout");
        assert!(scene.width > 0.0);
        assert!(scene.height > 0.0);
    }

    #[test]
    fn state_layout_determinism() {
        let mut sd = kozue_ir::StateDiagram::new();
        sd.states.insert("a".into(), kozue_ir::State::new("a", "A"));
        sd.states.insert("b".into(), kozue_ir::State::new("b", "B"));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::Initial,
            kozue_ir::Endpoint::State("a".into()),
            None,
        ));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::State("a".into()),
            kozue_ir::Endpoint::State("b".into()),
            None,
        ));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::State("b".into()),
            kozue_ir::Endpoint::Final,
            None,
        ));
        let scene1 = layout(&Diagram::State(sd.clone())).unwrap();
        let scene2 = layout(&Diagram::State(sd)).unwrap();
        assert_eq!(scene1, scene2, "state layout must be deterministic");
    }

    #[test]
    fn state_self_transition_does_not_panic() {
        let mut sd = kozue_ir::StateDiagram::new();
        sd.states.insert("s".into(), kozue_ir::State::new("s", "S"));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::Initial,
            kozue_ir::Endpoint::State("s".into()),
            None,
        ));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::State("s".into()),
            kozue_ir::Endpoint::State("s".into()),
            Some("self".to_string()),
        ));
        let scene = layout(&Diagram::State(sd)).expect("self-transition must not panic");
        assert!(scene.width > 0.0);
    }

    #[test]
    fn state_named_like_pseudostate_sentinel_is_not_corrupted() {
        // Regression: a real state named `__initial__` must NOT collide with the
        // synthetic pseudostate marker — pseudostate roles are keyed by index,
        // not by matching a magic id string, so it renders as a normal box.
        let mut sd = kozue_ir::StateDiagram::new();
        sd.states.insert(
            "__initial__".into(),
            kozue_ir::State::new("__initial__", "Real"),
        );
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::Initial,
            kozue_ir::Endpoint::State("__initial__".into()),
            None,
        ));
        let scene = layout(&Diagram::State(sd)).expect("layout");
        // Exactly one real state → exactly one Rect; its label survives.
        let rects = scene
            .items
            .iter()
            .filter(|i| matches!(i, SceneItem::Rect(_)))
            .count();
        assert_eq!(
            rects, 1,
            "real state must render as a box, not a pseudostate"
        );
        assert!(
            scene
                .items
                .iter()
                .any(|i| matches!(i, SceneItem::Text(t) if t.content == "Real")),
            "real state label must survive"
        );
    }

    // --- SemanticLayout (layout_full) contract tests ---

    /// `layout_full().scene` must be byte-for-byte the same Scene as `layout()`,
    /// and the semantic graph layout must mirror the scene: nodes in declaration
    /// order with the same rects, edge label anchor at the same point as the
    /// scene Text item.
    #[test]
    fn semantic_graph_matches_scene() {
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "A"));
        g.nodes.insert("b".into(), node("b", "B"));
        g.edges
            .push(Edge::new("a", "b", Some("go".into()), ArrowType::Triangle));
        let d = Diagram::Graph(g);

        let out = layout_full(&d).expect("layout_full");
        assert_eq!(out.scene, layout(&d).unwrap(), "scene must be unchanged");

        let SemanticLayout::Graph(sem) = &out.semantic else {
            panic!("expected SemanticLayout::Graph");
        };
        // Nodes: declaration order, rects identical to the scene rects.
        let ids: Vec<&str> = sem.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        let scene_rects = rects(&out.scene);
        assert_eq!(sem.nodes.len(), scene_rects.len());
        for (nl, r) in sem.nodes.iter().zip(&scene_rects) {
            assert_eq!(&&nl.rect, r, "semantic node rect must match scene rect");
        }
        // Edge: declaration index, endpoints, label anchor == scene Text position.
        assert_eq!(sem.edges.len(), 1);
        let el = &sem.edges[0];
        assert_eq!(el.index, 0);
        assert_eq!(el.from.id.as_str(), "a");
        assert_eq!(el.to.id.as_str(), "b");
        assert!(el.route.len() >= 2);
        let anchor = el.label_anchor.as_ref().expect("labeled edge has anchor");
        let text = out
            .scene
            .items
            .iter()
            .find_map(|i| match i {
                SceneItem::Text(t) if t.content == "go" => Some(t),
                _ => None,
            })
            .expect("edge label text in scene");
        assert_eq!((anchor.x, anchor.y), (text.x, text.y));
    }

    /// State transitions must come back in declaration order even though the
    /// layout emits regular transitions and self-transitions in separate passes.
    #[test]
    fn semantic_state_transitions_are_in_declaration_order() {
        let mut sd = kozue_ir::StateDiagram::new();
        sd.states.insert("a".into(), kozue_ir::State::new("a", "A"));
        sd.states.insert("b".into(), kozue_ir::State::new("b", "B"));
        // Declaration order: self-loop first, then regular, then to-final.
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::State("a".into()),
            kozue_ir::Endpoint::State("a".into()),
            Some("again".into()),
        ));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::State("a".into()),
            kozue_ir::Endpoint::State("b".into()),
            None,
        ));
        sd.transitions.push(kozue_ir::Transition::new(
            kozue_ir::Endpoint::State("b".into()),
            kozue_ir::Endpoint::Final,
            None,
        ));
        let out = layout_full(&Diagram::State(sd)).expect("layout_full");
        let SemanticLayout::State(sem) = &out.semantic else {
            panic!("expected SemanticLayout::State");
        };
        let indices: Vec<usize> = sem.transitions.iter().map(|t| t.index).collect();
        assert_eq!(
            indices,
            [0, 1, 2],
            "transitions must be in declaration order"
        );
        assert_eq!(
            sem.transitions[0].from,
            semantic::StateEndpointId::State("a".into())
        );
        assert_eq!(
            sem.transitions[0].to,
            semantic::StateEndpointId::State("a".into())
        );
        assert_eq!(sem.transitions[2].to, semantic::StateEndpointId::Final);
        assert!(sem.initial.is_none());
        assert!(sem.final_state.is_some());
        let states: Vec<&str> = sem.states.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(states, ["a", "b"]);
    }

    /// Sequence message `index` is the index into `SequenceDiagram::items`,
    /// and participants come back in declaration order.
    #[test]
    fn semantic_sequence_message_index_is_item_index() {
        let mut seq = kozue_ir::SequenceDiagram::new();
        seq_participant(&mut seq, "a", "Alice");
        seq_participant(&mut seq, "b", "Bob");
        seq_message(&mut seq, "a", "b", Some("hi"));
        seq_message(&mut seq, "b", "b", None); // self-message
        let out = layout_full(&Diagram::Sequence(seq)).expect("layout_full");
        let SemanticLayout::Sequence(sem) = &out.semantic else {
            panic!("expected SemanticLayout::Sequence");
        };
        let pids: Vec<&str> = sem.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(pids, ["a", "b"]);
        for p in &sem.participants {
            assert!(p.lifeline_y1 > p.lifeline_y0);
        }
        assert_eq!(sem.messages.len(), 2);
        assert_eq!(sem.messages[0].index, 0);
        assert_eq!(sem.messages[0].from.as_str(), "a");
        assert_eq!(sem.messages[0].to.as_str(), "b");
        assert!(sem.messages[0].label_anchor.is_some());
        assert_eq!(sem.messages[1].index, 1);
        assert!(sem.messages[1].label_anchor.is_none());
        assert!(
            sem.messages[1].route.len() >= 3,
            "self-message route is a multi-segment polyline"
        );
    }

    // --- Class / ER diagram layout smoke tests (Phase A) ---

    fn sample_class_diagram() -> kozue_ir::ClassDiagram {
        use kozue_ir::{ClassNode, ClassRelation, EndMarker};

        let mut cd = kozue_ir::ClassDiagram::new(Direction::Down);

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

        // Dog --|> Animal (inheritance: hollow triangle at the `to` end).
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
        // Kennel *-- Dog (composition: filled diamond at the `from` end),
        // with multiplicities and a label to exercise that code path too.
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

        cd
    }

    fn sample_er_diagram() -> kozue_ir::ErDiagram {
        use kozue_ir::{EndMarker, ErAttribute, ErEntity, ErRelation};

        let mut ed = kozue_ir::ErDiagram::new();

        let mut customer = ErEntity::new("Customer", "CUSTOMER");
        customer
            .attributes
            .push(ErAttribute::new("int", "id", vec!["PK".to_string()], None));
        customer
            .attributes
            .push(ErAttribute::new("string", "name", vec![], None));
        ed.entities.insert("Customer".into(), customer);

        let mut order = ErEntity::new("Order", "ORDER");
        order.attributes.push(ErAttribute::new(
            "int",
            "customer_id",
            vec!["FK".to_string()],
            Some("references Customer".to_string()),
        ));
        ed.entities.insert("Order".into(), order);

        // CUSTOMER ||--o{ ORDER : places
        ed.relations.push(ErRelation::new(
            "Customer",
            "Order",
            EndMarker::ErOne,
            EndMarker::ErZeroOrMany,
            Some("places".to_string()),
            kozue_ir::LineStyle::Solid,
        ));

        ed
    }

    #[test]
    fn class_layout_does_not_panic_and_renders_svg() {
        let diagram = Diagram::Class(sample_class_diagram());
        let out = layout_full(&diagram).expect("class layout must succeed");
        assert!(out.scene.width > 0.0 && out.scene.height > 0.0);

        // Three class boxes -> three outer Rects.
        assert_eq!(rects(&out.scene).len(), 3);

        // Section-divider lines: Animal has 2 non-empty sections -> 1 divider
        // (title|attrs) + 1 (attrs|methods) = 2. Dog has 1 section -> 1
        // divider. Kennel has 1 section -> 1 divider. Total 4 two-point
        // dividers among the open paths.
        let dividers = open_paths(&out.scene)
            .into_iter()
            .filter(|p| p.points.len() == 2)
            .count();
        // At least the 4 compartment dividers must be present (relation
        // lines and the diamond marker's back edge may also be 2-point
        // paths, so this is a lower bound, not an exact count).
        assert!(dividers >= 4, "expected >=4 divider lines, got {dividers}");

        // Hollow triangle marker: a closed (repeated endpoint), unfilled
        // 4-point path.
        let triangle = open_paths(&out.scene)
            .into_iter()
            .find(|p| p.points.len() == 4 && p.points[0] == p.points[3]);
        assert!(
            triangle.is_some(),
            "expected a closed hollow-triangle marker path"
        );

        // Filled diamond marker: a closed (repeated endpoint), filled
        // 5-point path.
        let diamond = filled_paths(&out.scene)
            .into_iter()
            .find(|p| p.points.len() == 5 && p.points[0] == p.points[4]);
        assert!(
            diamond.is_some(),
            "expected a closed filled-diamond marker path"
        );

        let SemanticLayout::Class(sem) = &out.semantic else {
            panic!("expected SemanticLayout::Class");
        };
        assert_eq!(sem.boxes.len(), 3);
        assert_eq!(sem.relations.len(), 2);
        let ids: Vec<&str> = sem.boxes.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["Animal", "Dog", "Kennel"]);
        let animal = &sem.boxes[0];
        assert_eq!(animal.compartments.len(), 2, "attrs + methods");

        let svg = kozue_render_svg::render(&out.scene);
        assert!(svg.starts_with("<svg"));
        assert!(!svg.is_empty());
    }

    #[test]
    fn er_layout_does_not_panic_and_renders_svg() {
        let diagram = Diagram::Er(sample_er_diagram());
        let out = layout_full(&diagram).expect("er layout must succeed");
        assert!(out.scene.width > 0.0 && out.scene.height > 0.0);

        // Two entities -> two outer Rects.
        assert_eq!(rects(&out.scene).len(), 2);

        // Crow's foot ("many" marker on the Order end): two 2-point open
        // paths fanning out from the tip, plus the ER "one" bar tick on the
        // Customer end (another 2-point open path).
        let two_point_paths = open_paths(&out.scene)
            .into_iter()
            .filter(|p| p.points.len() == 2)
            .count();
        assert!(
            two_point_paths >= 3,
            "expected >=3 two-point paths (crow's foot x2 + bar), got {two_point_paths}"
        );
        // The zero-or-many marker also draws a hollow circle (closed ring:
        // first point repeated at the end, >2 points).
        let circle = open_paths(&out.scene)
            .into_iter()
            .find(|p| p.points.len() > 2 && p.points.first() == p.points.last());
        assert!(circle.is_some(), "expected a hollow circle marker path");

        let SemanticLayout::Er(sem) = &out.semantic else {
            panic!("expected SemanticLayout::Er");
        };
        assert_eq!(sem.boxes.len(), 2);
        assert_eq!(sem.relations.len(), 1);
        assert_eq!(sem.relations[0].from.as_str(), "Customer");
        assert_eq!(sem.relations[0].to.as_str(), "Order");
        assert_eq!(sem.boxes[0].compartments.len(), 1, "single column section");

        let svg = kozue_render_svg::render(&out.scene);
        assert!(svg.starts_with("<svg"));
        assert!(!svg.is_empty());
    }

    #[test]
    fn class_and_er_layout_are_deterministic() {
        let class_diagram = Diagram::Class(sample_class_diagram());
        let out1 = layout_full(&class_diagram).unwrap();
        let out2 = layout_full(&class_diagram).unwrap();
        assert_eq!(out1.scene, out2.scene, "class scene must be deterministic");
        assert_eq!(
            kozue_render_svg::render(&out1.scene),
            kozue_render_svg::render(&out2.scene),
            "class SVG must be byte-identical across runs"
        );

        let er_diagram = Diagram::Er(sample_er_diagram());
        let out1 = layout_full(&er_diagram).unwrap();
        let out2 = layout_full(&er_diagram).unwrap();
        assert_eq!(out1.scene, out2.scene, "er scene must be deterministic");
        assert_eq!(
            kozue_render_svg::render(&out1.scene),
            kozue_render_svg::render(&out2.scene),
            "er SVG must be byte-identical across runs"
        );
    }

    #[test]
    fn class_self_relation_is_error() {
        use kozue_ir::{ClassNode, ClassRelation, EndMarker};
        let mut cd = kozue_ir::ClassDiagram::new(Direction::Down);
        cd.classes.insert("A".into(), ClassNode::new("A", "A"));
        cd.relations.push(ClassRelation::new(
            "A",
            "A",
            EndMarker::None,
            EndMarker::HollowTriangle,
            kozue_ir::LineStyle::Solid,
            None,
            None,
            None,
        ));
        let result = layout_full(&Diagram::Class(cd));
        assert!(result.is_err(), "self relations must be rejected");
    }
}
