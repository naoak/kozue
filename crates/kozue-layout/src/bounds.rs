//! Scene bounds computation and normalization.
//!
//! The layout is the single source of truth for scene bounds: it computes the
//! bounding box over every item (including text extents, edge labels and
//! arrowheads), translates all items so the top-left corner is at the origin,
//! and stores the resulting extent in `Scene.width`/`Scene.height`. The SVG
//! renderer uses those values directly.

use kozue_ir::{SceneItem, TextAlign};

/// Compute the bounding box `(min_x, min_y, max_x, max_y)` over all items.
///
/// Returns all zeros for an empty scene.
pub(crate) fn scene_bounds(items: &[SceneItem]) -> (f64, f64, f64, f64) {
    let mut bx = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for item in items {
        visit(item, &mut bx);
    }
    if bx.0.is_finite() {
        bx
    } else {
        (0.0, 0.0, 0.0, 0.0)
    }
}

fn visit(item: &SceneItem, bx: &mut (f64, f64, f64, f64)) {
    match item {
        SceneItem::Rect(r) => {
            bx.0 = bx.0.min(r.x);
            bx.1 = bx.1.min(r.y);
            bx.2 = bx.2.max(r.x + r.width);
            bx.3 = bx.3.max(r.y + r.height);
        }
        SceneItem::Path(p) => {
            for (x, y) in &p.points {
                bx.0 = bx.0.min(*x);
                bx.1 = bx.1.min(*y);
                bx.2 = bx.2.max(*x);
                bx.3 = bx.3.max(*y);
            }
        }
        SceneItem::Text(t) => {
            // Account for text extent based on alignment; `y` is the baseline.
            let (left_x, right_x) = match t.align {
                TextAlign::Start => (t.x, t.x + t.text_width),
                TextAlign::Middle => (t.x - t.text_width / 2.0, t.x + t.text_width / 2.0),
                TextAlign::End => (t.x - t.text_width, t.x),
                _ => (t.x, t.x + t.text_width), // future variants
            };
            bx.0 = bx.0.min(left_x);
            bx.1 = bx.1.min(t.y - t.text_height);
            bx.2 = bx.2.max(right_x);
            bx.3 = bx.3.max(t.y);
        }
        SceneItem::Group(g) => {
            for child in &g.items {
                visit(child, bx);
            }
        }
        _ => {} // future SceneItem variants: no bounds contribution
    }
}

/// Translate every item by `(dx, dy)`.
pub(crate) fn translate(items: &mut [SceneItem], dx: f64, dy: f64) {
    for item in items {
        translate_item(item, dx, dy);
    }
}

fn translate_item(item: &mut SceneItem, dx: f64, dy: f64) {
    match item {
        SceneItem::Rect(r) => {
            r.x += dx;
            r.y += dy;
        }
        SceneItem::Path(p) => {
            for (x, y) in &mut p.points {
                *x += dx;
                *y += dy;
            }
        }
        SceneItem::Text(t) => {
            t.x += dx;
            t.y += dy;
        }
        SceneItem::Group(g) => {
            for child in &mut g.items {
                translate_item(child, dx, dy);
            }
        }
        _ => {} // future SceneItem variants
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kozue_ir::Rect;

    #[test]
    fn empty_scene_has_zero_bounds() {
        assert_eq!(scene_bounds(&[]), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn translate_moves_rects() {
        let mut items = vec![SceneItem::Rect(Rect {
            x: 5.0,
            y: 7.0,
            width: 10.0,
            height: 10.0,
            rx: 0.0,
        })];
        translate(&mut items, -5.0, -7.0);
        let (min_x, min_y, max_x, max_y) = scene_bounds(&items);
        assert_eq!((min_x, min_y, max_x, max_y), (0.0, 0.0, 10.0, 10.0));
    }
}
