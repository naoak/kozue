//! Naive layered layout for M0.
//!
//! Nodes are assigned to layers by the longest-path method, arranged within
//! each layer in declaration order with equal spacing. No crossing reduction.

use indexmap::IndexMap;
use kozue_ir::{
    Diagram, Direction, Edge, GraphDiagram, Path, Rect, Scene, SceneItem, Text, TextAlign,
};

const FONT_SIZE: f64 = 16.0;
const PAD_X: f64 = 20.0;
const PAD_Y: f64 = 10.0;
const NODE_GAP: f64 = 40.0; // gap between nodes within a layer
const LAYER_GAP_DOWN: f64 = 100.0;
const LAYER_GAP_RIGHT: f64 = 150.0;
const ARROW_LEN: f64 = 10.0;
const ARROW_HALF_W: f64 = 5.0;

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
struct Placed {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    label: String,
}

impl Placed {
    fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// Lay out a semantic [`Diagram`] into a [`Scene`].
///
/// Returns `Err` if the diagram contains cycles (not yet supported) or other
/// structural problems.
pub fn layout(diagram: &Diagram) -> Result<Scene, LayoutError> {
    match diagram {
        Diagram::Graph(g) => layout_graph(g),
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

    let layers = assign_layers(g, &ids, &index_of)?;
    let max_layer = layers.iter().copied().max().unwrap_or(0);

    // Group node indices by layer, preserving declaration order.
    let mut by_layer: Vec<Vec<usize>> = vec![Vec::new(); max_layer + 1];
    for (i, &l) in layers.iter().enumerate() {
        by_layer[l].push(i);
    }

    // Measure each node's box.
    let boxes: Vec<(f64, f64, String)> = ids
        .iter()
        .map(|id| {
            let node = &g.nodes[*id];
            let (tw, th) = kozue_text::measure(&node.label, FONT_SIZE);
            (tw + 2.0 * PAD_X, th + 2.0 * PAD_Y, node.label.clone())
        })
        .collect();

    // The cross-axis extent of the widest layer, used to center layers.
    //
    // For direction=down  (horizontal_axis=false): cross axis is X,
    //   nodes are arranged side-by-side horizontally → sum widths.
    // For direction=right (horizontal_axis=true):  cross axis is Y,
    //   nodes are stacked vertically → sum heights.
    let cross_extent = |layer: &[usize], horizontal: bool| -> f64 {
        if layer.is_empty() {
            return 0.0;
        }
        let sizes: f64 = layer
            .iter()
            .map(|&i| if horizontal { boxes[i].1 } else { boxes[i].0 })
            .sum();
        sizes + NODE_GAP * (layer.len() as f64 - 1.0)
    };

    let horizontal_axis = g.direction == Direction::Right;
    let max_cross = by_layer
        .iter()
        .map(|layer| cross_extent(layer, horizontal_axis))
        .fold(0.0_f64, f64::max);

    // Place nodes.
    let mut placed: Vec<Placed> = Vec::with_capacity(ids.len());
    for _ in 0..ids.len() {
        placed.push(Placed {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            label: String::new(),
        });
    }

    let mut main_cursor = 0.0_f64;
    for layer in &by_layer {
        // Determine per-layer main-axis size (max along main axis).
        let layer_main = layer
            .iter()
            .map(|&i| {
                if horizontal_axis {
                    boxes[i].0
                } else {
                    boxes[i].1
                }
            })
            .fold(0.0_f64, f64::max);

        let extent = cross_extent(layer, horizontal_axis);
        let mut cross_cursor = (max_cross - extent) / 2.0;

        for &i in layer {
            let (bw, bh, ref label) = boxes[i];
            let (x, y) = if horizontal_axis {
                // direction right: main axis is x, cross axis is y.
                (main_cursor, cross_cursor)
            } else {
                // direction down: main axis is y, cross axis is x.
                (cross_cursor, main_cursor)
            };
            placed[i] = Placed {
                x,
                y,
                width: bw,
                height: bh,
                label: label.clone(),
            };
            cross_cursor += if horizontal_axis { bh } else { bw } + NODE_GAP;
        }

        let gap = if horizontal_axis {
            LAYER_GAP_RIGHT
        } else {
            LAYER_GAP_DOWN
        };
        main_cursor += layer_main + gap;
    }

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

    for edge in &g.edges {
        push_edge(&mut items, edge, &placed, &index_of);
    }

    // Compute bounds from placed node boxes.
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in &placed {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x + p.width);
        max_y = max_y.max(p.y + p.height);
    }
    if !min_x.is_finite() {
        min_x = 0.0;
        min_y = 0.0;
        max_x = 0.0;
        max_y = 0.0;
    }

    Ok(Scene {
        width: max_x - min_x,
        height: max_y - min_y,
        items,
    })
}

/// Assign a layer to each node using the longest path from any source.
///
/// Returns `Err` if a cycle is detected.
fn assign_layers(
    g: &GraphDiagram,
    ids: &[&String],
    index_of: &IndexMap<&str, usize>,
) -> Result<Vec<usize>, LayoutError> {
    let n = ids.len();
    // Adjacency + in-degree.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg: Vec<usize> = vec![0; n];
    for e in &g.edges {
        if let (Some(&from), Some(&to)) =
            (index_of.get(e.from.as_str()), index_of.get(e.to.as_str()))
        {
            adj[from].push(to);
            indeg[to] += 1;
        }
    }

    // Kahn topological order (declaration order among ready nodes).
    let mut layer = vec![0usize; n];
    let mut remaining = indeg.clone();
    let mut processed = vec![false; n];
    let mut count = 0;
    while count < n {
        // Pick lowest-index node with remaining in-degree 0.
        let mut picked = None;
        for i in 0..n {
            if !processed[i] && remaining[i] == 0 {
                picked = Some(i);
                break;
            }
        }
        let Some(u) = picked else {
            // Cycle detected: not yet supported.
            return Err(LayoutError {
                message: "cycles are not yet supported (planned for M1)".to_string(),
            });
        };
        processed[u] = true;
        count += 1;
        for &v in &adj[u] {
            if layer[u] + 1 > layer[v] {
                layer[v] = layer[u] + 1;
            }
            remaining[v] -= 1;
        }
    }

    Ok(layer)
}

fn push_edge(
    items: &mut Vec<SceneItem>,
    edge: &Edge,
    placed: &[Placed],
    index_of: &IndexMap<&str, usize>,
) {
    let (Some(&fi), Some(&ti)) = (
        index_of.get(edge.from.as_str()),
        index_of.get(edge.to.as_str()),
    ) else {
        return;
    };
    let from = &placed[fi];
    let to = &placed[ti];
    let (fx, fy) = from.center();
    let (tx, ty) = to.center();

    // Clip endpoints to the box borders.
    let start = clip_to_rect(from, tx, ty);
    let end = clip_to_rect(to, fx, fy);

    // Pull the line back so the tip of the arrow sits on the border.
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let ux = dx / len;
    let uy = dy / len;
    let line_end = (end.0 - ux * ARROW_LEN, end.1 - uy * ARROW_LEN);

    items.push(SceneItem::Path(Path {
        points: vec![start, line_end],
        filled: false,
    }));

    // Arrowhead triangle at `end`, pointing along (ux, uy).
    let base = line_end;
    let px = -uy; // perpendicular
    let py = ux;
    let left = (base.0 + px * ARROW_HALF_W, base.1 + py * ARROW_HALF_W);
    let right = (base.0 - px * ARROW_HALF_W, base.1 - py * ARROW_HALF_W);
    items.push(SceneItem::Path(Path {
        points: vec![end, left, right],
        filled: true,
    }));

    if let Some(label) = &edge.label {
        let mx = (start.0 + end.0) / 2.0;
        let my = (start.1 + end.1) / 2.0;
        let (tw, th) = kozue_text::measure(label, FONT_SIZE * 0.85);
        items.push(SceneItem::Text(Text {
            x: mx,
            y: my - 4.0,
            size: FONT_SIZE * 0.85,
            align: TextAlign::Middle,
            content: label.clone(),
            text_width: tw,
            text_height: th,
        }));
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
    use kozue_ir::{ArrowType, Node};

    fn node(id: &str, label: &str) -> Node {
        Node::new(id, label)
    }

    #[test]
    fn chain_layers_increase() {
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "A"));
        g.nodes.insert("b".into(), node("b", "B"));
        g.nodes.insert("c".into(), node("c", "C"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::Triangle));
        g.edges.push(Edge::new("b", "c", None, ArrowType::Triangle));
        let ids: Vec<&String> = g.nodes.keys().collect();
        let index_of: IndexMap<&str, usize> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let layers = assign_layers(&g, &ids, &index_of).expect("no cycle");
        assert_eq!(layers, vec![0, 1, 2]);
    }

    #[test]
    fn scene_has_positive_bounds() {
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "A"));
        g.nodes.insert("b".into(), node("b", "B"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::Triangle));
        let scene = layout(&Diagram::Graph(g)).expect("no cycle");
        assert!(scene.width > 0.0);
        assert!(scene.height > 0.0);
    }

    /// direction=down で同一層に複数ノードが並ぶ場合の cross_extent
    /// (cross軸=X方向) は幅の和+GAP になること。
    #[test]
    fn cross_extent_down_multi_node_layer() {
        // Two nodes with no edges: both end up in layer 0.
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "Alpha"));
        g.nodes.insert("b".into(), node("b", "Beta"));
        // No edges → both in layer 0.
        let scene = layout(&Diagram::Graph(g)).expect("no cycle");
        // With direction=down, cross axis is X. Two nodes side by side.
        // The scene width must be > width of a single node.
        let (single_w, _) = kozue_text::measure("Alpha", FONT_SIZE);
        let single_box_w = single_w + 2.0 * PAD_X;
        assert!(
            scene.width > single_box_w,
            "scene width {} should exceed single node box width {}",
            scene.width,
            single_box_w
        );
    }

    /// direction=right で同一層に複数ノードが並ぶ場合の cross_extent
    /// (cross軸=Y方向) は高さの和+GAP になること。
    #[test]
    fn cross_extent_right_multi_node_layer() {
        // Two nodes with no edges: both end up in layer 0.
        let mut g = GraphDiagram::new(Direction::Right);
        g.nodes.insert("a".into(), node("a", "Alpha"));
        g.nodes.insert("b".into(), node("b", "Beta"));
        // No edges → both in layer 0.
        let scene = layout(&Diagram::Graph(g)).expect("no cycle");
        // With direction=right, cross axis is Y. Two nodes stacked vertically.
        let (_, single_h) = kozue_text::measure("Alpha", FONT_SIZE);
        let single_box_h = single_h + 2.0 * PAD_Y;
        assert!(
            scene.height > single_box_h,
            "scene height {} should exceed single node box height {}",
            scene.height,
            single_box_h
        );
    }

    #[test]
    fn cycle_returns_error() {
        let mut g = GraphDiagram::new(Direction::Down);
        g.nodes.insert("a".into(), node("a", "A"));
        g.nodes.insert("b".into(), node("b", "B"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::Triangle));
        g.edges.push(Edge::new("b", "a", None, ArrowType::Triangle));
        let result = layout(&Diagram::Graph(g));
        assert!(result.is_err());
        let msg = result.unwrap_err().message;
        assert!(
            msg.contains("cycles are not yet supported"),
            "unexpected error: {msg}"
        );
    }
}
