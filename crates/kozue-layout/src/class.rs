//! Class-diagram layout: compartment boxes (name/attributes/methods) placed
//! with the shared layered (Sugiyama-style) pipeline, connected by relation
//! lines carrying UML end markers.

use indexmap::IndexMap;
use kozue_ir::{ClassDiagram, ElementId, Path, Scene, SceneItem, StrokeWeight, Text, TextAlign};

use crate::boxes::{self, BoxSpec, ROW_FONT_SIZE};
use crate::markers;
use crate::semantic::{ClassLayout, CompartmentBox, RelationLayout, SemanticLayout};
use crate::{
    bounds, coords, cycle, edge_label_anchor, layering, ordering, parallel_edge_offsets,
    LayoutError, Placed, LAYER_GAP_DOWN, LAYER_GAP_RIGHT, NODE_GAP,
};

/// Gap between a multiplicity label and the point it annotates.
const MULT_PERP_OFFSET: f64 = 8.0;
/// Extra clearance beyond the marker's own shrink for the multiplicity label.
const MULT_ALONG_GAP: f64 = 6.0;
const MULT_FONT_SIZE: f64 = ROW_FONT_SIZE;

pub(crate) fn layout_class_full(c: &ClassDiagram) -> Result<crate::LayoutOutput, LayoutError> {
    let ids: Vec<&ElementId> = c.classes.keys().collect();
    let index_of: IndexMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let n = ids.len();

    let mut raw_edges: Vec<(usize, usize)> = Vec::new();
    let mut rel_ids: Vec<usize> = Vec::new();
    for (i, r) in c.relations.iter().enumerate() {
        crate::validate_marker(r.from_marker)?;
        crate::validate_marker(r.to_marker)?;
        crate::validate_line(r.line)?;
        let (Some(&from), Some(&to)) = (index_of.get(r.from.as_str()), index_of.get(r.to.as_str()))
        else {
            return Err(LayoutError {
                message: format!(
                    "class relation references unknown class ({} -> {})",
                    r.from, r.to
                ),
            });
        };
        if from == to {
            return Err(LayoutError {
                message: format!("self relations are not supported ({} -> {})", r.from, r.to),
            });
        }
        raw_edges.push((from, to));
        rel_ids.push(i);
    }

    let reversed = cycle::greedy_reversed(n, &raw_edges);
    let acyclic: Vec<(usize, usize)> = raw_edges
        .iter()
        .zip(&reversed)
        .map(|(&(u, v), &r)| if r { (v, u) } else { (u, v) })
        .collect();
    let layers = layering::longest_path(n, &acyclic);

    let specs: Vec<BoxSpec> = ids
        .iter()
        .map(|id| {
            let node = &c.classes[*id];
            let mut sections = Vec::new();
            if !node.attributes.is_empty() {
                sections.push(node.attributes.clone());
            }
            if !node.methods.is_empty() {
                sections.push(node.methods.clone());
            }
            boxes::measure(
                node.id.clone(),
                node.name.clone(),
                node.stereotype.clone(),
                sections,
            )
        })
        .collect();

    let (horizontal, reverse_main) = crate::direction_axes(c.direction)?;
    let sizes: Vec<(f64, f64)> = specs
        .iter()
        .map(|s| {
            if horizontal {
                (s.height, s.width)
            } else {
                (s.width, s.height)
            }
        })
        .collect();

    let mut lay = ordering::build(n, &sizes, &layers, &acyclic);
    ordering::reduce_crossings(&mut lay);
    let cross = coords::assign_cross(&lay, NODE_GAP);

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
    let total_main = if nl == 0 { 0.0 } else { cursor - layer_gap };

    let placed: Vec<Placed> = (0..n)
        .map(|v| {
            let (w, h) = (specs[v].width, specs[v].height);
            let main_size = if horizontal { w } else { h };
            let main = crate::orient_main_start(
                layer_start[lay.layer_of[v]],
                main_size,
                total_main,
                reverse_main,
            );
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
                label: specs[v].title.clone(),
                kind: kozue_ir::NodeKind::Default,
            }
        })
        .collect();

    let point_of = |v: usize| -> (f64, f64) {
        if lay.is_dummy[v] {
            let l = lay.layer_of[v];
            let forward = layer_start[l] + layer_size[l] / 2.0;
            let main = crate::orient_main_center(forward, total_main, reverse_main);
            if horizontal {
                (main, cross[v])
            } else {
                (cross[v], main)
            }
        } else {
            placed[v].center()
        }
    };

    let mut items: Vec<SceneItem> = Vec::new();
    let mut sem_boxes: Vec<CompartmentBox> = Vec::new();
    for (v, p) in placed.iter().enumerate() {
        let (box_items, sem_box) = boxes::emit(&specs[v], p.x, p.y);
        items.extend(box_items);
        sem_boxes.push(sem_box);
    }

    let offsets = parallel_edge_offsets(&raw_edges, &placed);
    let mut sem_relations: Vec<RelationLayout> = Vec::new();

    for (k, &(from, to)) in raw_edges.iter().enumerate() {
        let mut pts: Vec<(f64, f64)> = lay.chains[k].iter().map(|&v| point_of(v)).collect();
        if reversed[k] {
            pts.reverse();
        }
        crate::bow_polyline(&mut pts, offsets[k]);
        let rel = &c.relations[rel_ids[k]];

        let geom = crate::compute_edge_geom(pts, &placed[from], &placed[to], None, None)?;
        let route = geom.route;
        let last = route.len() - 1;

        let dir_from = unit(sub(route[0], route[1]));
        let dir_to = unit(sub(route[last], route[last - 1]));

        let shrink_from = markers::push_end_marker(&mut items, rel.from_marker, route[0], dir_from);
        let shrink_to = markers::push_end_marker(&mut items, rel.to_marker, route[last], dir_to);

        let mut line_pts = route.clone();
        line_pts[0] = (
            route[0].0 - dir_from.0 * shrink_from,
            route[0].1 - dir_from.1 * shrink_from,
        );
        line_pts[last] = (
            route[last].0 - dir_to.0 * shrink_to,
            route[last].1 - dir_to.1 * shrink_to,
        );
        items.push(SceneItem::Path(Path {
            points: line_pts,
            filled: false,
            stroke: crate::line_style_to_stroke(rel.line),
            weight: StrokeWeight::Normal,
        }));

        if let Some(label) = rel.label.as_deref() {
            let (tw, th) = kozue_text::measure(label, ROW_FONT_SIZE);
            let (lx, ly) = edge_label_anchor(&route, tw, th, offsets[k]);
            items.push(SceneItem::Text(Text {
                x: lx,
                y: ly,
                size: ROW_FONT_SIZE,
                align: TextAlign::Middle,
                content: label.to_string(),
                text_width: tw,
                text_height: th,
            }));
        }

        if let Some(m) = rel.from_mult.as_deref() {
            push_mult_label(&mut items, m, route[0], dir_from, shrink_from);
        }
        if let Some(m) = rel.to_mult.as_deref() {
            push_mult_label(&mut items, m, route[last], dir_to, shrink_to);
        }

        sem_relations.push(RelationLayout {
            index: rel_ids[k],
            from: rel.from.clone(),
            to: rel.to.clone(),
            points: route,
            from_marker: rel.from_marker,
            to_marker: rel.to_marker,
            line: rel.line,
            label: rel.label.clone(),
            from_mult: rel.from_mult.clone(),
            to_mult: rel.to_mult.clone(),
        });
    }
    sem_relations.sort_by_key(|r| r.index);

    let (min_x, min_y, max_x, max_y) = bounds::scene_bounds(&items);
    bounds::translate(&mut items, -min_x, -min_y);

    for b in &mut sem_boxes {
        b.rect.x -= min_x;
        b.rect.y -= min_y;
        for comp in &mut b.compartments {
            comp.top_y -= min_y;
        }
    }
    for r in &mut sem_relations {
        for pt in &mut r.points {
            pt.0 -= min_x;
            pt.1 -= min_y;
        }
    }

    let scene = Scene {
        width: max_x - min_x,
        height: max_y - min_y,
        items,
    };
    let semantic = SemanticLayout::Class(ClassLayout {
        width: max_x - min_x,
        height: max_y - min_y,
        boxes: sem_boxes,
        relations: sem_relations,
    });

    Ok(crate::LayoutOutput { scene, semantic })
}

fn sub(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 - b.0, a.1 - b.1)
}

fn unit(v: (f64, f64)) -> (f64, f64) {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt().max(1e-6);
    (v.0 / len, v.1 / len)
}

/// Draw a multiplicity label just past the marker at one end of a relation
/// line, offset perpendicular to the line so it doesn't overlap the marker.
fn push_mult_label(
    items: &mut Vec<SceneItem>,
    label: &str,
    tip: (f64, f64),
    dir: (f64, f64),
    shrink: f64,
) {
    let (px, py) = (-dir.1, dir.0);
    let along = shrink + MULT_ALONG_GAP;
    let (tw, th) = kozue_text::measure(label, MULT_FONT_SIZE);
    items.push(SceneItem::Text(Text {
        x: tip.0 - dir.0 * along + px * MULT_PERP_OFFSET,
        y: tip.1 - dir.1 * along + py * MULT_PERP_OFFSET,
        size: MULT_FONT_SIZE,
        align: TextAlign::Middle,
        content: label.to_string(),
        text_width: tw,
        text_height: th,
    }));
}
