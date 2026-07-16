//! Relation end-markers shared by class-diagram and ER-diagram layout.
//!
//! Every marker is drawn from a `tip` point (where the relation line meets
//! the box border) and a `dir` unit vector: the direction the line travels
//! *as it arrives* at that box (i.e. pointing from the line's interior
//! toward the box). This is the same convention [`crate::push_edge_geom`]
//! uses for its arrowhead. The marker geometry is built backward from `tip`
//! along `-dir`, so it always sits just outside the box border, pointing in.
//!
//! Hollow shapes (`HollowTriangle`, `HollowDiamond`, ER bar/circle/crow) are
//! closed unfilled [`Path`]s (`filled: false`); solid shapes (`FilledDiamond`)
//! are closed filled `Path`s. This mirrors the existing Scene IR convention
//! for arrowheads (see the M1 module doc) so the SVG/PNG/term renderers need
//! no changes.

use kozue_ir::{EndMarker, Path, SceneItem, StrokeStyle, StrokeWeight};

use crate::circle_path;

const TRIANGLE_LEN: f64 = 14.0;
const TRIANGLE_HALF_W: f64 = 7.0;
const DIAMOND_LEN: f64 = 16.0;
const DIAMOND_HALF_W: f64 = 6.0;
const OPEN_ARROW_LEN: f64 = 10.0;
const OPEN_ARROW_HALF_W: f64 = 5.0;
const ER_BAR_DIST: f64 = 10.0;
const ER_BAR_HALF_W: f64 = 6.0;
const ER_CROW_LEN: f64 = 12.0;
const ER_CROW_HALF_W: f64 = 6.0;
const ER_CIRCLE_R: f64 = 5.0;
const ER_CIRCLE_GAP: f64 = 2.0;

/// Draw the end marker `m` at `tip`, oriented by `dir` (unit vector, line
/// travel direction arriving at `tip`). Returns the length by which the
/// connecting line should be shortened at this end so it doesn't poke through
/// the marker glyph (0.0 for markers that are meant to be crossed by the
/// line, e.g. [`EndMarker::None`] and [`EndMarker::OpenArrow`]).
pub(crate) fn push_end_marker(
    items: &mut Vec<SceneItem>,
    m: EndMarker,
    tip: (f64, f64),
    dir: (f64, f64),
) -> f64 {
    let (ux, uy) = dir;
    let (px, py) = (-uy, ux);
    match m {
        EndMarker::None => 0.0,
        EndMarker::HollowTriangle => {
            let base = (tip.0 - ux * TRIANGLE_LEN, tip.1 - uy * TRIANGLE_LEN);
            let left = (base.0 + px * TRIANGLE_HALF_W, base.1 + py * TRIANGLE_HALF_W);
            let right = (base.0 - px * TRIANGLE_HALF_W, base.1 - py * TRIANGLE_HALF_W);
            items.push(SceneItem::Path(Path {
                points: vec![tip, left, right, tip],
                filled: false,
                stroke: StrokeStyle::Solid,
                weight: StrokeWeight::Normal,
            }));
            TRIANGLE_LEN
        }
        EndMarker::FilledDiamond | EndMarker::HollowDiamond => {
            let filled = matches!(m, EndMarker::FilledDiamond);
            let mid = (
                tip.0 - ux * DIAMOND_LEN / 2.0,
                tip.1 - uy * DIAMOND_LEN / 2.0,
            );
            let back = (tip.0 - ux * DIAMOND_LEN, tip.1 - uy * DIAMOND_LEN);
            let left = (mid.0 + px * DIAMOND_HALF_W, mid.1 + py * DIAMOND_HALF_W);
            let right = (mid.0 - px * DIAMOND_HALF_W, mid.1 - py * DIAMOND_HALF_W);
            items.push(SceneItem::Path(Path {
                points: vec![tip, left, back, right, tip],
                filled,
                stroke: StrokeStyle::Solid,
                weight: StrokeWeight::Normal,
            }));
            DIAMOND_LEN
        }
        EndMarker::OpenArrow => {
            let back = (tip.0 - ux * OPEN_ARROW_LEN, tip.1 - uy * OPEN_ARROW_LEN);
            let left = (
                back.0 + px * OPEN_ARROW_HALF_W,
                back.1 + py * OPEN_ARROW_HALF_W,
            );
            let right = (
                back.0 - px * OPEN_ARROW_HALF_W,
                back.1 - py * OPEN_ARROW_HALF_W,
            );
            items.push(SceneItem::Path(Path {
                points: vec![left, tip, right],
                filled: false,
                stroke: StrokeStyle::Solid,
                weight: StrokeWeight::Normal,
            }));
            0.0
        }
        EndMarker::ErOne => {
            push_bar(items, tip, (ux, uy), (px, py), ER_BAR_DIST);
            ER_BAR_DIST
        }
        EndMarker::ErMany => {
            push_crow(items, tip, (ux, uy), (px, py));
            ER_CROW_LEN
        }
        EndMarker::ErZeroOrOne => {
            push_bar(items, tip, (ux, uy), (px, py), ER_BAR_DIST);
            let dist = ER_BAR_DIST + ER_CIRCLE_GAP + ER_CIRCLE_R;
            let center = (tip.0 - ux * dist, tip.1 - uy * dist);
            items.push(SceneItem::Path(circle_path(
                center.0,
                center.1,
                ER_CIRCLE_R,
                false,
            )));
            dist + ER_CIRCLE_R
        }
        EndMarker::ErOneOrMany => {
            push_crow(items, tip, (ux, uy), (px, py));
            push_bar(items, tip, (ux, uy), (px, py), ER_CROW_LEN);
            ER_CROW_LEN
        }
        EndMarker::ErZeroOrMany => {
            push_crow(items, tip, (ux, uy), (px, py));
            let dist = ER_CROW_LEN + ER_CIRCLE_GAP + ER_CIRCLE_R;
            let center = (tip.0 - ux * dist, tip.1 - uy * dist);
            items.push(SceneItem::Path(circle_path(
                center.0,
                center.1,
                ER_CIRCLE_R,
                false,
            )));
            dist + ER_CIRCLE_R
        }
        // `EndMarker` is `#[non_exhaustive]`: treat any future variant as
        // undecorated (no marker drawn, no line shortening) rather than panic.
        _ => 0.0,
    }
}

/// A short tick perpendicular to the relation line, `dist` back from `tip`
/// (the ER "exactly one" bar).
fn push_bar(
    items: &mut Vec<SceneItem>,
    tip: (f64, f64),
    dir: (f64, f64),
    perp: (f64, f64),
    dist: f64,
) {
    let (ux, uy) = dir;
    let (px, py) = perp;
    let center = (tip.0 - ux * dist, tip.1 - uy * dist);
    let a = (center.0 + px * ER_BAR_HALF_W, center.1 + py * ER_BAR_HALF_W);
    let b = (center.0 - px * ER_BAR_HALF_W, center.1 - py * ER_BAR_HALF_W);
    items.push(SceneItem::Path(Path {
        points: vec![a, b],
        filled: false,
        stroke: StrokeStyle::Solid,
        weight: StrokeWeight::Normal,
    }));
}

/// Crow's foot: two diagonal strokes fanning from `tip` back to a point
/// `ER_CROW_LEN` behind it (the ER "many" marker). Two separate `Path`s
/// (plus the line itself, drawn by the caller) form the classic 3-pronged
/// crow's foot.
fn push_crow(items: &mut Vec<SceneItem>, tip: (f64, f64), dir: (f64, f64), perp: (f64, f64)) {
    let (ux, uy) = dir;
    let (px, py) = perp;
    let back = (tip.0 - ux * ER_CROW_LEN, tip.1 - uy * ER_CROW_LEN);
    let left = (back.0 + px * ER_CROW_HALF_W, back.1 + py * ER_CROW_HALF_W);
    let right = (back.0 - px * ER_CROW_HALF_W, back.1 - py * ER_CROW_HALF_W);
    items.push(SceneItem::Path(Path {
        points: vec![tip, left],
        filled: false,
        stroke: StrokeStyle::Solid,
        weight: StrokeWeight::Normal,
    }));
    items.push(SceneItem::Path(Path {
        points: vec![tip, right],
        filled: false,
        stroke: StrokeStyle::Solid,
        weight: StrokeWeight::Normal,
    }));
}
