//! Compartment-box geometry shared by class-diagram and ER-diagram layout.
//!
//! A "compartment box" is a title row (optionally preceded by a
//! `<<stereotype>>` line) followed by zero or more horizontally-divided
//! sections of left-aligned text rows (attributes, methods, ER columns...).
//! Sections with no rows are omitted entirely (no divider, no empty area).
//!
//! [`measure`] computes the box's size from its content alone (needed before
//! layout knows the box's final position); [`emit`] draws the box's Scene
//! items and the matching [`CompartmentBox`] once the position is known. The
//! two share the same per-block height accounting so they can never diverge.

use kozue_ir::{ElementId, Path, Rect, SceneItem, Text, TextAlign};

use crate::semantic::{Compartment, CompartmentBox};
use crate::{FONT_SIZE, PAD_X};

/// Font size used for compartment row text (attributes/methods/columns) and
/// the stereotype line, smaller than the title per UML convention.
pub(crate) const ROW_FONT_SIZE: f64 = FONT_SIZE * 0.85;
/// Vertical padding above/below each row's text within its compartment.
const ROW_PAD_Y: f64 = 6.0;
/// Vertical padding above/below the title block's content.
const TITLE_PAD_Y: f64 = 8.0;
/// Gap between the stereotype line and the title line.
const STEREOTYPE_GAP: f64 = 2.0;

/// A measured compartment box, ready to be placed at an (x, y) origin.
pub(crate) struct BoxSpec {
    pub(crate) id: ElementId,
    pub(crate) title: String,
    pub(crate) stereotype: Option<String>,
    /// Non-empty sections only, top-to-bottom order.
    pub(crate) sections: Vec<Vec<String>>,
    pub(crate) width: f64,
    pub(crate) height: f64,
    /// Height of the title block (stereotype + title + padding), i.e. the
    /// y-offset (relative to the box top) where the first compartment starts.
    title_block_h: f64,
}

/// Measure a compartment box's size from its title/stereotype/section rows.
///
/// `sections` may contain empty inner `Vec`s; those are dropped (an empty
/// section contributes no divider and no area).
pub(crate) fn measure(
    id: impl Into<ElementId>,
    title: impl Into<String>,
    stereotype: Option<String>,
    sections: Vec<Vec<String>>,
) -> BoxSpec {
    let id = id.into();
    let title = title.into();
    let sections: Vec<Vec<String>> = sections.into_iter().filter(|s| !s.is_empty()).collect();

    let (title_w, title_h) = kozue_text::measure(&title, FONT_SIZE);
    let stereo_line = stereotype.as_ref().map(|s| format!("«{s}»"));
    let stereo_dims = stereo_line
        .as_ref()
        .map(|s| kozue_text::measure(s, ROW_FONT_SIZE));

    let mut max_w = title_w;
    if let Some((sw, _)) = stereo_dims {
        max_w = max_w.max(sw);
    }
    for row in sections.iter().flatten() {
        let (rw, _) = kozue_text::measure(row, ROW_FONT_SIZE);
        max_w = max_w.max(rw);
    }
    let width = max_w + 2.0 * PAD_X;

    let title_block_h = TITLE_PAD_Y
        + stereo_dims
            .map(|(_, sh)| sh + STEREOTYPE_GAP)
            .unwrap_or(0.0)
        + title_h
        + TITLE_PAD_Y;

    let mut height = title_block_h;
    for row in sections.iter().flatten() {
        let (_, rh) = kozue_text::measure(row, ROW_FONT_SIZE);
        height += rh + 2.0 * ROW_PAD_Y;
    }

    BoxSpec {
        id,
        title,
        stereotype,
        sections,
        width,
        height,
        title_block_h,
    }
}

/// Emit the Scene items (outer rect, section dividers, text rows) and the
/// matching [`CompartmentBox`] for a box placed at `(x, y)`.
pub(crate) fn emit(spec: &BoxSpec, x: f64, y: f64) -> (Vec<SceneItem>, CompartmentBox) {
    let mut items: Vec<SceneItem> = Vec::new();
    items.push(SceneItem::Rect(Rect {
        x,
        y,
        width: spec.width,
        height: spec.height,
        rx: 0.0,
    }));

    let cx = x + spec.width / 2.0;
    let mut cy = y + TITLE_PAD_Y;
    if let Some(stereo) = &spec.stereotype {
        let line = format!("«{stereo}»");
        let (sw, sh) = kozue_text::measure(&line, ROW_FONT_SIZE);
        items.push(SceneItem::Text(Text {
            x: cx,
            y: cy + sh * 0.8,
            size: ROW_FONT_SIZE,
            align: TextAlign::Middle,
            content: line,
            text_width: sw,
            text_height: sh,
        }));
        cy += sh + STEREOTYPE_GAP;
    }
    let (tw, th) = kozue_text::measure(&spec.title, FONT_SIZE);
    items.push(SceneItem::Text(Text {
        x: cx,
        y: cy + th * 0.8,
        size: FONT_SIZE,
        align: TextAlign::Middle,
        content: spec.title.clone(),
        text_width: tw,
        text_height: th,
    }));

    let mut compartments: Vec<Compartment> = Vec::new();
    let mut row_y = y + spec.title_block_h;
    if !spec.sections.is_empty() {
        items.push(divider(x, row_y, spec.width));
    }
    for (si, section) in spec.sections.iter().enumerate() {
        let top_y = row_y;
        for row in section {
            let (rw, rh) = kozue_text::measure(row, ROW_FONT_SIZE);
            row_y += ROW_PAD_Y;
            items.push(SceneItem::Text(Text {
                x: x + PAD_X,
                y: row_y + rh * 0.8,
                size: ROW_FONT_SIZE,
                align: TextAlign::Start,
                content: row.clone(),
                text_width: rw,
                text_height: rh,
            }));
            row_y += rh + ROW_PAD_Y;
        }
        compartments.push(Compartment {
            top_y,
            rows: section.clone(),
        });
        if si + 1 < spec.sections.len() {
            items.push(divider(x, row_y, spec.width));
        }
    }

    let sem = CompartmentBox {
        id: spec.id.clone(),
        rect: Rect {
            x,
            y,
            width: spec.width,
            height: spec.height,
            rx: 0.0,
        },
        title: spec.title.clone(),
        stereotype: spec.stereotype.clone(),
        compartments,
    };
    (items, sem)
}

fn divider(x: f64, y: f64, width: f64) -> SceneItem {
    SceneItem::Path(Path {
        points: vec![(x, y), (x + width, y)],
        filled: false,
        dashed: false,
    })
}
