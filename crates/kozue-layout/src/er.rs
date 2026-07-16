//! ER-diagram layout: entity boxes (name + attribute rows) placed with the
//! shared layered (Sugiyama-style) pipeline, connected by relation lines
//! carrying crow's-foot end markers.
//!
//! Structurally this mirrors [`crate::class`] closely (both are "compartment
//! box + two-marker relation" diagrams); [`ErDiagram`] has no direction
//! field, so entities are always laid out top-down.

use indexmap::IndexMap;
use kozue_ir::{
    ElementId, ErAttribute, ErDiagram, Path, Scene, SceneItem, StrokeWeight, Text, TextAlign,
};

use crate::boxes::{self, BoxSpec, ROW_FONT_SIZE};
use crate::markers;
use crate::semantic::{ClassLayout, CompartmentBox, RelationLayout, SemanticLayout};
use crate::{
    bounds, coords, cycle, edge_label_anchor, layering, ordering, parallel_edge_offsets,
    LayoutError, Placed, LAYER_GAP_DOWN, NODE_GAP,
};

pub(crate) fn layout_er_full(e: &ErDiagram) -> Result<crate::LayoutOutput, LayoutError> {
    let ids: Vec<&ElementId> = e.entities.keys().collect();
    let index_of: IndexMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let n = ids.len();

    let mut raw_edges: Vec<(usize, usize)> = Vec::new();
    let mut rel_ids: Vec<usize> = Vec::new();
    for (i, r) in e.relations.iter().enumerate() {
        crate::validate_marker(r.from_marker)?;
        crate::validate_marker(r.to_marker)?;
        crate::validate_line(r.line)?;
        let (Some(&from), Some(&to)) = (index_of.get(r.from.as_str()), index_of.get(r.to.as_str()))
        else {
            return Err(LayoutError {
                message: format!(
                    "ER relation references unknown entity ({} -> {})",
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
            let entity = &e.entities[*id];
            let mut sections = Vec::new();
            if !entity.attributes.is_empty() {
                sections.push(entity.attributes.iter().map(format_attr).collect());
            }
            boxes::measure(entity.id.clone(), entity.name.clone(), None, sections)
        })
        .collect();

    // ER diagrams have no direction field: always top-down.
    let sizes: Vec<(f64, f64)> = specs.iter().map(|s| (s.width, s.height)).collect();

    let mut lay = ordering::build(n, &sizes, &layers, &acyclic);
    ordering::reduce_crossings(&mut lay);
    let cross = coords::assign_cross(&lay, NODE_GAP);

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

    let placed: Vec<Placed> = (0..n)
        .map(|v| {
            let (w, h) = (specs[v].width, specs[v].height);
            let main = layer_start[lay.layer_of[v]];
            Placed {
                x: cross[v] - w / 2.0,
                y: main,
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
            let main = layer_start[l] + layer_size[l] / 2.0;
            (cross[v], main)
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
        let rel = &e.relations[rel_ids[k]];

        let geom = crate::compute_edge_geom(pts, &placed[from], &placed[to])?;
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

        sem_relations.push(RelationLayout {
            index: rel_ids[k],
            from: rel.from.clone(),
            to: rel.to.clone(),
            points: route,
            from_marker: rel.from_marker,
            to_marker: rel.to_marker,
            line: rel.line,
            label: rel.label.clone(),
            from_mult: None,
            to_mult: None,
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
    let semantic = SemanticLayout::Er(ClassLayout {
        width: max_x - min_x,
        height: max_y - min_y,
        boxes: sem_boxes,
        relations: sem_relations,
    });

    Ok(crate::LayoutOutput { scene, semantic })
}

fn format_attr(a: &ErAttribute) -> String {
    let mut s = String::new();
    if !a.keys.is_empty() {
        s.push('[');
        s.push_str(&a.keys.join(","));
        s.push_str("] ");
    }
    s.push_str(&a.name);
    if !a.type_name.is_empty() {
        s.push_str(": ");
        s.push_str(&a.type_name);
    }
    if let Some(c) = &a.comment {
        s.push_str("  // ");
        s.push_str(c);
    }
    s
}

fn sub(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 - b.0, a.1 - b.1)
}

fn unit(v: (f64, f64)) -> (f64, f64) {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt().max(1e-6);
    (v.0 / len, v.1 / len)
}
