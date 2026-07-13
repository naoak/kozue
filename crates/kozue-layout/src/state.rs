//! State-diagram layout (M7a).

use kozue_ir::{ArrowType, Path, Rect, Scene, SceneItem, StateDiagram, Text, TextAlign};

use super::{
    bounds, coords, cycle, layering, ordering, LayoutError, Placed, ARROW_HALF_W, ARROW_LEN,
    FONT_SIZE, LAYER_GAP_DOWN, NODE_GAP, PAD_X, PAD_Y,
};

const STATE_CIRCLE_R: f64 = 8.0;
const STATE_FINAL_INNER_R: f64 = 6.0;
const STATE_FINAL_OUTER_R: f64 = 12.0;
const SELF_LOOP_OFFSET: f64 = 25.0;
const CIRCLE_POINTS: usize = 20;

fn circle_path(cx: f64, cy: f64, r: f64, filled: bool) -> Path {
    let mut points: Vec<(f64, f64)> = (0..CIRCLE_POINTS)
        .map(|i| {
            let angle = i as f64 * 2.0 * std::f64::consts::PI / CIRCLE_POINTS as f64;
            (cx + r * angle.cos(), cy + r * angle.sin())
        })
        .collect();
    // Close the ring: an unfilled circle renders as an open polyline, so repeat
    // the first point to join the last segment back to the start (otherwise the
    // stroked outer circle of a final state has a visible gap at angle 0).
    if let Some(&first) = points.first() {
        points.push(first);
    }
    Path {
        points,
        filled,
        dashed: false,
    }
}

pub(crate) fn layout_state(diagram: &StateDiagram) -> Result<Scene, LayoutError> {
    let mut node_ids: Vec<String> = Vec::new();
    let mut node_labels: Vec<String> = Vec::new();

    // Track seen IDs to avoid duplicates.
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // Add explicitly declared states first (in insertion order).
    for (id, state) in &diagram.states {
        if seen.insert(id.clone()) {
            node_ids.push(id.clone());
            node_labels.push(state.label.clone());
        }
    }

    // Scan transitions for auto-declared states and pseudostates.
    let mut has_initial = false;
    let mut has_final = false;
    for t in &diagram.transitions {
        if let kozue_ir::Endpoint::State(id) = &t.from {
            if seen.insert(id.clone()) {
                node_ids.push(id.clone());
                node_labels.push(id.clone());
            }
        }
        if let kozue_ir::Endpoint::State(id) = &t.to {
            if seen.insert(id.clone()) {
                node_ids.push(id.clone());
                node_labels.push(id.clone());
            }
        }
        if matches!(t.from, kozue_ir::Endpoint::Initial) {
            has_initial = true;
        }
        if matches!(t.to, kozue_ir::Endpoint::Final) {
            has_final = true;
        }
    }

    // Synthetic pseudostate indices.
    let initial_idx = if has_initial {
        let idx = node_ids.len();
        node_ids.push("__initial__".to_string());
        node_labels.push(String::new());
        Some(idx)
    } else {
        None
    };
    let final_idx = if has_final {
        let idx = node_ids.len();
        node_ids.push("__final__".to_string());
        node_labels.push(String::new());
        Some(idx)
    } else {
        None
    };

    let n = node_ids.len();
    if n == 0 {
        return Ok(Scene {
            width: 0.0,
            height: 0.0,
            items: Vec::new(),
        });
    }

    // Build index map: id -> index. Pseudostate markers are addressed by index
    // (`initial_idx`/`final_idx`), never by id, so they are NOT inserted here —
    // this keeps a real state that happens to be named `__initial__`/`__final__`
    // from being overwritten or mis-routed. Roles are decided by index, not by
    // matching a magic id string, so such a state renders as a normal state.
    let mut index_of: indexmap::IndexMap<String, usize> = indexmap::IndexMap::new();
    for (i, id) in node_ids.iter().enumerate() {
        if Some(i) == initial_idx || Some(i) == final_idx {
            continue;
        }
        index_of.insert(id.clone(), i);
    }

    // Build sizes for each node: (cross_size, main_size) = (width, height) for direction=down.
    let boxes: Vec<(f64, f64)> = (0..node_ids.len())
        .map(|i| {
            if Some(i) == initial_idx {
                (STATE_CIRCLE_R * 2.0, STATE_CIRCLE_R * 2.0)
            } else if Some(i) == final_idx {
                (STATE_FINAL_OUTER_R * 2.0, STATE_FINAL_OUTER_R * 2.0)
            } else {
                let (tw, th) = kozue_text::measure(&node_labels[i], FONT_SIZE);
                (tw + 2.0 * PAD_X, th + 2.0 * PAD_Y)
            }
        })
        .collect();

    // Build raw edges (skip self-transitions — handled separately).
    let mut raw_edges: Vec<(usize, usize)> = Vec::new();
    let mut self_trans_indices: Vec<usize> = Vec::new();
    let mut edge_to_trans: Vec<usize> = Vec::new(); // raw_edge_index -> transition_index

    for (ti, t) in diagram.transitions.iter().enumerate() {
        let from_idx = match &t.from {
            kozue_ir::Endpoint::Initial => initial_idx,
            kozue_ir::Endpoint::Final => None,
            kozue_ir::Endpoint::State(id) => index_of.get(id).copied(),
            _ => None,
        };
        let to_idx = match &t.to {
            kozue_ir::Endpoint::Final => final_idx,
            kozue_ir::Endpoint::Initial => None,
            kozue_ir::Endpoint::State(id) => index_of.get(id).copied(),
            _ => None,
        };
        let (Some(from), Some(to)) = (from_idx, to_idx) else {
            continue;
        };
        if from == to {
            self_trans_indices.push(ti);
        } else {
            raw_edges.push((from, to));
            edge_to_trans.push(ti);
        }
    }

    // Phase 1: cycle removal.
    let reversed = cycle::greedy_reversed(n, &raw_edges);
    let acyclic: Vec<(usize, usize)> = raw_edges
        .iter()
        .zip(&reversed)
        .map(|(&(u, v), &r)| if r { (v, u) } else { (u, v) })
        .collect();

    // Phase 2: layer assignment.
    let layers = layering::longest_path(n, &acyclic);

    // Phase 3: dummy insertion + crossing reduction.
    let mut lay = ordering::build(n, &boxes, &layers, &acyclic);
    ordering::reduce_crossings(&mut lay);

    // Phase 4: coordinate assignment.
    let cross = coords::assign_cross(&lay, NODE_GAP);

    // Main-axis positions per layer.
    let nl = lay.order.len();
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
        cursor += size + LAYER_GAP_DOWN;
    }

    // Place real nodes (direction=down).
    let placed: Vec<Placed> = (0..n)
        .map(|v| {
            let (w, h) = boxes[v];
            let main = layer_start[lay.layer_of[v]];
            let x = cross[v] - w / 2.0;
            let y = main;
            Placed {
                x,
                y,
                width: w,
                height: h,
                label: node_labels[v].clone(),
            }
        })
        .collect();

    // Routing point for any node: real node center or dummy point.
    let point_of = |v: usize| -> (f64, f64) {
        if lay.is_dummy[v] {
            let l = lay.layer_of[v];
            let main = layer_start[l] + layer_size[l] / 2.0;
            (cross[v], main)
        } else {
            placed[v].center()
        }
    };

    // Build scene items.
    let mut items: Vec<SceneItem> = Vec::new();

    // Emit each real node. Roles are keyed by index, not id string.
    for (v, p) in placed.iter().enumerate() {
        if Some(v) == initial_idx {
            let (cx, cy) = p.center();
            items.push(SceneItem::Path(circle_path(cx, cy, STATE_CIRCLE_R, true)));
        } else if Some(v) == final_idx {
            let (cx, cy) = p.center();
            items.push(SceneItem::Path(circle_path(
                cx,
                cy,
                STATE_FINAL_INNER_R,
                true,
            )));
            items.push(SceneItem::Path(circle_path(
                cx,
                cy,
                STATE_FINAL_OUTER_R,
                false,
            )));
        } else {
            // Regular state: rounded rect + label.
            items.push(SceneItem::Rect(Rect {
                x: p.x,
                y: p.y,
                width: p.width,
                height: p.height,
                rx: 6.0,
            }));
            let (cx, cy) = p.center();
            let (tw, th) = kozue_text::measure(&p.label, FONT_SIZE);
            items.push(SceneItem::Text(Text {
                x: cx,
                y: cy + FONT_SIZE * 0.35,
                size: FONT_SIZE,
                align: TextAlign::Middle,
                content: p.label.clone(),
                text_width: tw,
                text_height: th,
            }));
        }
    }

    // Separate mutual transitions (e.g. `a -> b` and `b -> a`) so they render
    // apart instead of coincident.
    let offsets = super::parallel_edge_offsets(&raw_edges, &placed);
    // Emit regular transitions.
    for (k, &(from, to)) in raw_edges.iter().enumerate() {
        let mut pts: Vec<(f64, f64)> = lay.chains[k].iter().map(|&v| point_of(v)).collect();
        if reversed[k] {
            pts.reverse();
        }
        super::bow_polyline(&mut pts, offsets[k]);
        let trans_label = diagram.transitions[edge_to_trans[k]].label.as_deref();
        super::push_edge(
            &mut items,
            pts,
            &placed[from],
            &placed[to],
            trans_label,
            ArrowType::Triangle,
        );
    }

    // Emit self-transitions.
    for &ti in &self_trans_indices {
        let t = &diagram.transitions[ti];
        let state_id = match &t.from {
            kozue_ir::Endpoint::State(id) => id,
            _ => continue,
        };
        let Some(&v) = index_of.get(state_id) else {
            continue;
        };
        let p = &placed[v];
        let (_, cy) = p.center();
        let box_right = p.x + p.width;

        // Self-loop: exits right side, loops around.
        let loop_pts: Vec<(f64, f64)> = vec![
            (box_right, cy - 5.0),
            (box_right + SELF_LOOP_OFFSET, cy - SELF_LOOP_OFFSET),
            (box_right + SELF_LOOP_OFFSET, cy + 5.0),
            (box_right, cy + 5.0),
        ];

        let last = loop_pts.len() - 1;
        let end = loop_pts[last];
        let dx = end.0 - loop_pts[last - 1].0;
        let dy = end.1 - loop_pts[last - 1].1;
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let ux = dx / len;
        let uy = dy / len;
        let line_end = (end.0 - ux * ARROW_LEN, end.1 - uy * ARROW_LEN);

        let mut line_pts = loop_pts.clone();
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

        if let Some(label) = t.label.as_deref() {
            let (tw, th) = kozue_text::measure(label, FONT_SIZE * 0.85);
            items.push(SceneItem::Text(Text {
                x: box_right + SELF_LOOP_OFFSET / 2.0,
                y: cy - SELF_LOOP_OFFSET - 4.0,
                size: FONT_SIZE * 0.85,
                align: TextAlign::Middle,
                content: label.to_string(),
                text_width: tw,
                text_height: th,
            }));
        }
    }

    // Normalize bounds.
    let (min_x, min_y, max_x, max_y) = bounds::scene_bounds(&items);
    bounds::translate(&mut items, -min_x, -min_y);

    Ok(Scene {
        width: max_x - min_x,
        height: max_y - min_y,
        items,
    })
}
