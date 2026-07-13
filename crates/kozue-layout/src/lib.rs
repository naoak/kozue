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
mod coords;
mod cycle;
mod layering;
mod ordering;
mod sequence;
mod state;

use indexmap::IndexMap;
use kozue_ir::{
    ArrowType, Diagram, Direction, GraphDiagram, Path, Rect, Scene, SceneItem, Text, TextAlign,
};

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

/// Lay out a semantic [`Diagram`] into a [`Scene`].
///
/// Cycles are supported: back edges are reversed internally for layering and
/// drawn in their original direction. Self-loop edges are rejected.
pub fn layout(diagram: &Diagram) -> Result<Scene, LayoutError> {
    match diagram {
        Diagram::Graph(g) => layout_graph(g),
        Diagram::Sequence(s) => Ok(sequence::layout_sequence(s)),
        Diagram::State(s) => state::layout_state(s),
        _ => Err(LayoutError {
            message: "unsupported diagram variant".to_string(),
        }),
    }
}

fn layout_graph(g: &GraphDiagram) -> Result<Scene, LayoutError> {
    // Node index order = declaration order.
    let ids: Vec<&String> = g.nodes.keys().collect();
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

    for p in &placed {
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
    }

    // Separate parallel/mutual edges so they don't draw on top of each other.
    let offsets = parallel_edge_offsets(&raw_edges, &placed);
    for (k, &(from, to)) in raw_edges.iter().enumerate() {
        // Chain points in layout (acyclic) orientation; restore the original
        // direction so the arrowhead points along the declared edge.
        let mut pts: Vec<(f64, f64)> = lay.chains[k].iter().map(|&v| point_of(v)).collect();
        if reversed[k] {
            pts.reverse();
        }
        bow_polyline(&mut pts, offsets[k]);
        let edge = &g.edges[edge_ids[k]];
        push_edge(
            &mut items,
            pts,
            &placed[from],
            &placed[to],
            edge.label.as_deref(),
            edge.arrow,
        );
    }

    // Normalize: layout owns the bounds, including text and arrowheads.
    let (min_x, min_y, max_x, max_y) = bounds::scene_bounds(&items);
    bounds::translate(&mut items, -min_x, -min_y);

    Ok(Scene {
        width: max_x - min_x,
        height: max_y - min_y,
        items,
    })
}

/// Draw one edge as a polyline through its bend points, with an optional
/// arrowhead at the target end and an optional label at the polyline midpoint.
///
/// `pts` are the edge's routing points in original edge orientation:
/// `[from_center, bends.., to_center]` (length >= 2).
/// Perpendicular spacing between parallel edges sharing the same node pair.
pub(crate) const EDGE_SEP: f64 = 14.0;

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

pub(crate) fn push_edge(
    items: &mut Vec<SceneItem>,
    mut pts: Vec<(f64, f64)>,
    from: &Placed,
    to: &Placed,
    label: Option<&str>,
    arrow: ArrowType,
) {
    let last = pts.len() - 1;
    // Clip the endpoints to the node borders.
    pts[0] = clip_to_rect(from, pts[1].0, pts[1].1);
    let end = clip_to_rect(to, pts[last - 1].0, pts[last - 1].1);
    pts[last] = end;

    let draw_arrow = !matches!(arrow, ArrowType::None);

    if draw_arrow {
        // Pull the final segment back so the tip of the arrow sits on the border.
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

        // Arrowhead triangle at `end`, pointing along (ux, uy).
        let px = -uy; // perpendicular
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
        // No arrowhead: draw the line all the way to the node border.
        items.push(SceneItem::Path(Path {
            points: pts.clone(),
            filled: false,
            dashed: false,
        }));
    }

    if let Some(label) = label {
        let (mx, my) = polyline_midpoint(&pts);
        let (tw, th) = kozue_text::measure(label, FONT_SIZE * 0.85);
        items.push(SceneItem::Text(Text {
            x: mx,
            y: my - 4.0,
            size: FONT_SIZE * 0.85,
            align: TextAlign::Middle,
            content: label.to_string(),
            text_width: tw,
            text_height: th,
        }));
    }
}

/// The point at half the arc length of a polyline.
fn polyline_midpoint(pts: &[(f64, f64)]) -> (f64, f64) {
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
}
