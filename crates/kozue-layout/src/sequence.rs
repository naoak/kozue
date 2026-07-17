//! Deterministic arithmetic layout for sequence diagrams.

use std::collections::HashMap;

use indexmap::IndexMap;
use kozue_ir::{
    ElementId, MessageArrow, NotePosition, ParticipantKind, Path, Rect, Scene, SceneItem,
    SequenceDiagram, SequenceItem, StrokeStyle, StrokeWeight, Text, TextAlign,
};

use crate::semantic::SequenceItemLayout;

use crate::bounds;
use crate::semantic;

const FONT_SIZE: f64 = 16.0;
const STEREOTYPE_FONT_SIZE: f64 = 12.0; // font size for «kind» stereotype text
const PAD_X: f64 = 20.0;
const PAD_Y: f64 = 10.0;
const STEREOTYPE_GAP: f64 = 2.0; // extra gap between stereotype line and label
const HEADER_RX: f64 = 4.0;
const MIN_COL_GAP: f64 = 80.0; // minimum gap between adjacent column centers
const MSG_LABEL_PAD: f64 = 16.0; // horizontal padding around message label
const MSG_ROW_HEIGHT: f64 = 48.0;
const SELF_MSG_WIDTH: f64 = 30.0;
const SELF_MSG_HEIGHT: f64 = 24.0;
const HEADER_TOP: f64 = 0.0;
const MSG_START_Y: f64 = 80.0; // y of first message line (below headers)
const LIFELINE_EXTRA: f64 = 24.0; // extra space below last message
const ARROW_LEN: f64 = 10.0;
const ARROW_HALF_W: f64 = 5.0;
const NOTE_PAD_X: f64 = 8.0; // horizontal padding inside a note box
const NOTE_PAD_Y: f64 = 6.0; // vertical padding inside a note box
const NOTE_GAP: f64 = 10.0; // gap between a note and its target lifeline (left/right of)
const NOTE_EAR: f64 = 8.0; // size of the folded corner on a note box
const NOTE_SPAN_EXTEND: f64 = 8.0; // extra half-width past the outer lifelines for span notes
const DIVIDER_PAD_X: f64 = 12.0; // horizontal padding inside a divider band
const DIVIDER_PAD_Y: f64 = 6.0; // vertical padding inside a divider band
const DIVIDER_EXTEND: f64 = 16.0; // extend of a divider band past the outer lifelines
const DELAY_EXTEND: f64 = 16.0; // extend of a delay's dotted line past the outer lifelines
const REF_EXTEND: f64 = 8.0; // extra half-width past the outer lifelines for a reference frame
const REF_TAB_W: f64 = 34.0; // width of the "ref" tab in a reference frame
const REF_TAB_H: f64 = 16.0; // height of the "ref" tab in a reference frame
const REF_PAD_Y: f64 = 8.0; // vertical padding inside a reference frame
const BAR_WIDTH: f64 = 10.0; // activation bar width
const BAR_NEST_OFFSET: f64 = 3.0; // offset per nesting depth

/// Returns the guillemet stereotype string for a non-Default participant kind.
/// Returns `None` for `Default`.
fn stereotype_label(kind: &ParticipantKind) -> Option<&'static str> {
    match kind {
        ParticipantKind::Default => None,
        ParticipantKind::Actor => Some("«actor»"),
        ParticipantKind::Boundary => Some("«boundary»"),
        ParticipantKind::Control => Some("«control»"),
        ParticipantKind::Entity => Some("«entity»"),
        ParticipantKind::Database => Some("«database»"),
        ParticipantKind::Collections => Some("«collections»"),
        ParticipantKind::Queue => Some("«queue»"),
        // Safe fallback for presentation-path renderers (svg/png/term).
        // Strict rejection of unrecognised `ParticipantKind` variants is the
        // responsibility of `validate_export_semantics` in the exchange-contract
        // layer; presentation paths are not the enforcement boundary.
        _ => Some("«participant»"),
    }
}

/// Shaft retraction (px) at a message end carrying the given marker: the
/// filled triangle covers the line end, so the shaft stops at its base; every
/// other marker is stroked on top of (or around) a full-length shaft.
fn shaft_retraction(arrow: MessageArrow) -> f64 {
    match arrow {
        MessageArrow::Filled => ARROW_LEN,
        _ => 0.0,
    }
}

/// Push the Scene items for a message end marker at `tip`. `dx` is the
/// horizontal unit direction pointing from the shaft toward the tip (message
/// glyphs are always horizontal: straight messages and the self-loop return
/// segment both run along x).
///
/// `Circle` is approximated with a small filled octagon because the Scene IR
/// has no circle/ellipse primitive yet — M4 adds a real ellipse primitive and
/// this approximation will be replaced then.
fn push_arrow_glyph(items: &mut Vec<SceneItem>, tip: (f64, f64), dx: f64, arrow: MessageArrow) {
    let (tx, ty) = tip;
    let bx = tx - dx * ARROW_LEN;
    match arrow {
        MessageArrow::None => {}
        MessageArrow::Filled => {
            items.push(SceneItem::Path(Path {
                points: vec![(tx, ty), (bx, ty - ARROW_HALF_W), (bx, ty + ARROW_HALF_W)],
                filled: true,
                stroke: StrokeStyle::Solid,
                weight: StrokeWeight::Normal,
            }));
        }
        MessageArrow::Open => {
            // Open V-stroke: two legs meeting at the tip, not filled.
            items.push(SceneItem::Path(Path {
                points: vec![(bx, ty - ARROW_HALF_W), (tx, ty), (bx, ty + ARROW_HALF_W)],
                filled: false,
                stroke: StrokeStyle::Solid,
                weight: StrokeWeight::Normal,
            }));
        }
        MessageArrow::Cross => {
            // X-stroke centered a half arrow-length before the tip.
            let cx = tx - dx * (ARROW_LEN / 2.0);
            items.push(SceneItem::Path(Path {
                points: vec![
                    (cx - ARROW_HALF_W, ty - ARROW_HALF_W),
                    (cx + ARROW_HALF_W, ty + ARROW_HALF_W),
                ],
                filled: false,
                stroke: StrokeStyle::Solid,
                weight: StrokeWeight::Normal,
            }));
            items.push(SceneItem::Path(Path {
                points: vec![
                    (cx - ARROW_HALF_W, ty + ARROW_HALF_W),
                    (cx + ARROW_HALF_W, ty - ARROW_HALF_W),
                ],
                filled: false,
                stroke: StrokeStyle::Solid,
                weight: StrokeWeight::Normal,
            }));
        }
        MessageArrow::Circle => {
            // Temporary approximation: filled octagon centered on the tip
            // (no circle primitive in the Scene IR until M4).
            let r = ARROW_HALF_W;
            let c = r * std::f64::consts::FRAC_1_SQRT_2;
            items.push(SceneItem::Path(Path {
                points: vec![
                    (tx + r, ty),
                    (tx + c, ty + c),
                    (tx, ty + r),
                    (tx - c, ty + c),
                    (tx - r, ty),
                    (tx - c, ty - c),
                    (tx, ty - r),
                    (tx + c, ty - c),
                ],
                filled: true,
                stroke: StrokeStyle::Solid,
                weight: StrokeWeight::Normal,
            }));
        }
        // `MessageArrow` is `#[non_exhaustive]`: future variants are rejected
        // by `validate_message_arrow` before drawing, so this arm is
        // unreachable in practice; draw nothing rather than panic.
        _ => {}
    }
}

/// Draw a note box (UML dog-eared rectangle outline + centered text) and push
/// its `NoteLayout`. `note_box_width` mirrors the width used during column
/// placement. Validation (target existence, target count) has already run.
#[allow(clippy::too_many_arguments)]
fn draw_note(
    items: &mut Vec<SceneItem>,
    sem_items: &mut Vec<SequenceItemLayout>,
    note: &kozue_ir::Note,
    row: usize,
    col_x: &[f64],
    idx_of: &IndexMap<&str, usize>,
    note_box_width: &impl Fn(&str) -> f64,
) {
    let y_center = MSG_START_Y + row as f64 * MSG_ROW_HEIGHT;
    let (tw, th) = kozue_text::measure(&note.text, FONT_SIZE);
    let text_w = tw + 2.0 * NOTE_PAD_X;
    let nh = th + 2.0 * NOTE_PAD_Y;

    let cols: Vec<f64> = note
        .targets
        .iter()
        .filter_map(|t| idx_of.get(t.as_str()).map(|&i| col_x[i]))
        .collect();
    // `cols` is non-empty: validated before drawing.
    let (x0, nw) = match note.position {
        NotePosition::Over => {
            let left = cols.iter().cloned().fold(f64::INFINITY, f64::min);
            let right = cols.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let center = (left + right) / 2.0;
            let span_w = (right - left) + 2.0 * NOTE_SPAN_EXTEND;
            let nw = text_w.max(span_w);
            (center - nw / 2.0, nw)
        }
        NotePosition::LeftOf => {
            let nw = note_box_width(&note.text);
            (cols[0] - NOTE_GAP - nw, nw)
        }
        NotePosition::RightOf => {
            let nw = note_box_width(&note.text);
            (cols[0] + NOTE_GAP, nw)
        }
        _ => (cols[0] - text_w / 2.0, text_w),
    };
    let y0 = y_center - nh / 2.0;
    let x1 = x0 + nw;
    let y1 = y0 + nh;

    // Outline with a folded top-right corner (dog-ear).
    items.push(SceneItem::Path(Path {
        points: vec![
            (x0, y0),
            (x1 - NOTE_EAR, y0),
            (x1, y0 + NOTE_EAR),
            (x1, y1),
            (x0, y1),
            (x0, y0),
        ],
        filled: false,
        stroke: StrokeStyle::Solid,
        weight: StrokeWeight::Normal,
    }));
    // The fold triangle.
    items.push(SceneItem::Path(Path {
        points: vec![
            (x1 - NOTE_EAR, y0),
            (x1 - NOTE_EAR, y0 + NOTE_EAR),
            (x1, y0 + NOTE_EAR),
        ],
        filled: false,
        stroke: StrokeStyle::Solid,
        weight: StrokeWeight::Normal,
    }));

    let cx = x0 + nw / 2.0;
    let cy = y0 + nh / 2.0;
    items.push(SceneItem::Text(Text {
        x: cx,
        y: cy + FONT_SIZE * 0.35,
        size: FONT_SIZE,
        align: TextAlign::Middle,
        content: note.text.clone(),
        text_width: tw,
        text_height: th,
    }));

    sem_items.push(SequenceItemLayout::Note(semantic::NoteLayout {
        index: row,
        text: note.text.clone(),
        position: note.position,
        targets: note.targets.clone(),
        rect: Rect {
            x: x0,
            y: y0,
            width: nw,
            height: nh,
            rx: 0.0,
        },
        text_anchor: semantic::Point::new(cx, cy),
    }));
}

/// Full-width diagram span `[x_left, x_right]` from the outer lifelines.
fn diagram_span(col_x: &[f64]) -> (f64, f64) {
    if col_x.is_empty() {
        (0.0, 0.0)
    } else {
        (col_x[0], col_x[col_x.len() - 1])
    }
}

fn draw_divider(
    items: &mut Vec<SceneItem>,
    sem_items: &mut Vec<SequenceItemLayout>,
    divider: &kozue_ir::Divider,
    row: usize,
    col_x: &[f64],
) {
    let y_center = MSG_START_Y + row as f64 * MSG_ROW_HEIGHT;
    let (x_left, x_right) = diagram_span(col_x);
    let (tw, th) = kozue_text::measure(&divider.text, FONT_SIZE);
    let center = (x_left + x_right) / 2.0;
    let band_w = (tw + 2.0 * DIVIDER_PAD_X).max((x_right - x_left) + 2.0 * DIVIDER_EXTEND);
    let band_h = th + 2.0 * DIVIDER_PAD_Y;
    let x0 = center - band_w / 2.0;
    let y0 = y_center - band_h / 2.0;

    items.push(SceneItem::Rect(Rect {
        x: x0,
        y: y0,
        width: band_w,
        height: band_h,
        rx: 0.0,
    }));
    let cy = y0 + band_h / 2.0;
    items.push(SceneItem::Text(Text {
        x: center,
        y: cy + FONT_SIZE * 0.35,
        size: FONT_SIZE,
        align: TextAlign::Middle,
        content: divider.text.clone(),
        text_width: tw,
        text_height: th,
    }));

    sem_items.push(SequenceItemLayout::Divider(semantic::DividerLayout {
        index: row,
        text: divider.text.clone(),
        rect: Rect {
            x: x0,
            y: y0,
            width: band_w,
            height: band_h,
            rx: 0.0,
        },
        text_anchor: semantic::Point::new(center, cy),
    }));
}

fn draw_delay(
    items: &mut Vec<SceneItem>,
    sem_items: &mut Vec<SequenceItemLayout>,
    delay: &kozue_ir::Delay,
    row: usize,
    col_x: &[f64],
) {
    let y_center = MSG_START_Y + row as f64 * MSG_ROW_HEIGHT;
    let (x_left, x_right) = diagram_span(col_x);
    let x0 = x_left - DELAY_EXTEND;
    let x1 = x_right + DELAY_EXTEND;

    // Dotted line crossing the full width at y_center.
    items.push(SceneItem::Path(Path {
        points: vec![(x0, y_center), (x1, y_center)],
        filled: false,
        stroke: StrokeStyle::Dotted,
        weight: StrokeWeight::Normal,
    }));

    let (rect_h, text_anchor) = if let Some(text) = &delay.text {
        let (tw, th) = kozue_text::measure(text, FONT_SIZE);
        let center = (x_left + x_right) / 2.0;
        items.push(SceneItem::Text(Text {
            x: center,
            y: y_center + FONT_SIZE * 0.35,
            size: FONT_SIZE,
            align: TextAlign::Middle,
            content: text.clone(),
            text_width: tw,
            text_height: th,
        }));
        (th, Some(semantic::Point::new(center, y_center)))
    } else {
        (0.0, None)
    };

    sem_items.push(SequenceItemLayout::Delay(semantic::DelayLayout {
        index: row,
        text: delay.text.clone(),
        rect: Rect {
            x: x0,
            y: y_center - rect_h / 2.0,
            width: x1 - x0,
            height: rect_h,
            rx: 0.0,
        },
        text_anchor,
    }));
}

fn draw_reference(
    items: &mut Vec<SceneItem>,
    sem_items: &mut Vec<SequenceItemLayout>,
    reference: &kozue_ir::Reference,
    row: usize,
    col_x: &[f64],
    idx_of: &IndexMap<&str, usize>,
) {
    let y_center = MSG_START_Y + row as f64 * MSG_ROW_HEIGHT;
    let cols: Vec<f64> = reference
        .targets
        .iter()
        .filter_map(|t| idx_of.get(t.as_str()).map(|&i| col_x[i]))
        .collect();
    // `cols` is non-empty: validated before drawing.
    let left = cols.iter().cloned().fold(f64::INFINITY, f64::min);
    let right = cols.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let center = (left + right) / 2.0;
    let (tw, th) = kozue_text::measure(&reference.text, FONT_SIZE);
    let span_w = (right - left) + 2.0 * REF_EXTEND;
    let frame_w = (tw + 2.0 * NOTE_PAD_X).max(span_w).max(REF_TAB_W * 2.0);
    let frame_h = th + REF_TAB_H + 2.0 * REF_PAD_Y;
    let x0 = center - frame_w / 2.0;
    let y0 = y_center - frame_h / 2.0;
    let y1 = y0 + frame_h;

    // Outer frame.
    items.push(SceneItem::Rect(Rect {
        x: x0,
        y: y0,
        width: frame_w,
        height: frame_h,
        rx: 0.0,
    }));
    // "ref" tab in the top-left corner.
    items.push(SceneItem::Path(Path {
        points: vec![
            (x0, y0 + REF_TAB_H),
            (x0 + REF_TAB_W, y0 + REF_TAB_H),
            (x0 + REF_TAB_W, y0),
        ],
        filled: false,
        stroke: StrokeStyle::Solid,
        weight: StrokeWeight::Normal,
    }));
    let (rtw, rth) = kozue_text::measure("ref", STEREOTYPE_FONT_SIZE);
    items.push(SceneItem::Text(Text {
        x: x0 + REF_TAB_W / 2.0,
        y: y0 + REF_TAB_H / 2.0 + STEREOTYPE_FONT_SIZE * 0.35,
        size: STEREOTYPE_FONT_SIZE,
        align: TextAlign::Middle,
        content: "ref".to_string(),
        text_width: rtw,
        text_height: rth,
    }));
    // Body text centered below the tab.
    let body_cy = (y0 + REF_TAB_H + y1) / 2.0;
    items.push(SceneItem::Text(Text {
        x: center,
        y: body_cy + FONT_SIZE * 0.35,
        size: FONT_SIZE,
        align: TextAlign::Middle,
        content: reference.text.clone(),
        text_width: tw,
        text_height: th,
    }));

    sem_items.push(SequenceItemLayout::Reference(semantic::ReferenceLayout {
        index: row,
        text: reference.text.clone(),
        targets: reference.targets.clone(),
        rect: Rect {
            x: x0,
            y: y0,
            width: frame_w,
            height: frame_h,
            rx: 0.0,
        },
        text_anchor: semantic::Point::new(center, body_cy),
    }));
}

pub(crate) fn layout_sequence_full(
    seq: &SequenceDiagram,
) -> Result<crate::LayoutOutput, crate::LayoutError> {
    for item in &seq.items {
        match item {
            SequenceItem::Message(message) => {
                if !seq.participants.contains_key(&message.from)
                    || !seq.participants.contains_key(&message.to)
                {
                    return Err(crate::LayoutError {
                        message: format!(
                            "sequence message references unknown participant ({} -> {})",
                            message.from, message.to
                        ),
                    });
                }
                crate::validate_line(message.line)?;
                crate::validate_message_arrow(message.head)?;
                crate::validate_message_arrow(message.tail)?;
            }
            SequenceItem::Note(note) => {
                if note.targets.is_empty() {
                    return Err(crate::LayoutError {
                        message: "sequence note has no target participants".to_string(),
                    });
                }
                match note.position {
                    NotePosition::LeftOf | NotePosition::RightOf => {
                        if note.targets.len() != 1 {
                            return Err(crate::LayoutError {
                                message:
                                    "sequence note `left of`/`right of` requires exactly one target"
                                        .to_string(),
                            });
                        }
                    }
                    NotePosition::Over => {}
                    // `NotePosition` is `#[non_exhaustive]`: reject unknown variants.
                    _ => {
                        return Err(crate::LayoutError {
                            message: "unsupported future sequence note position".to_string(),
                        })
                    }
                }
                for target in &note.targets {
                    if !seq.participants.contains_key(target) {
                        return Err(crate::LayoutError {
                            message: format!(
                                "sequence note references unknown participant ({target})"
                            ),
                        });
                    }
                }
            }
            // Dividers and delays carry only free-form text: nothing to validate.
            SequenceItem::Divider(_) => {}
            SequenceItem::Delay(_) => {}
            SequenceItem::Reference(reference) => {
                if reference.targets.is_empty() {
                    return Err(crate::LayoutError {
                        message: "sequence reference has no target participants".to_string(),
                    });
                }
                for target in &reference.targets {
                    if !seq.participants.contains_key(target) {
                        return Err(crate::LayoutError {
                            message: format!(
                                "sequence reference references unknown participant ({target})"
                            ),
                        });
                    }
                }
            }
            SequenceItem::Activate(a) | SequenceItem::Deactivate(a) => {
                if !seq.participants.contains_key(&a.participant) {
                    return Err(crate::LayoutError {
                        message: format!(
                            "sequence activation references unknown participant ({})",
                            a.participant
                        ),
                    });
                }
            }
            // `SequenceItem` is `#[non_exhaustive]`: reject unknown variants.
            _ => {
                return Err(crate::LayoutError {
                    message: "unsupported future sequence item".to_string(),
                })
            }
        }
    }
    // Collect participant IDs in insertion order.
    let ids: Vec<&ElementId> = seq.participants.keys().collect();
    let n = ids.len();

    // Measure each header box.
    let header_sizes: Vec<(f64, f64)> = ids
        .iter()
        .map(|id| {
            let p = &seq.participants[*id];
            let label = &p.label;
            let (tw, th) = kozue_text::measure(label, FONT_SIZE);
            if let Some(st) = stereotype_label(&p.kind) {
                let (stw, sth) = kozue_text::measure(st, STEREOTYPE_FONT_SIZE);
                let w = tw.max(stw) + 2.0 * PAD_X;
                let h = sth + STEREOTYPE_GAP + th + 2.0 * PAD_Y;
                (w, h)
            } else {
                (tw + 2.0 * PAD_X, th + 2.0 * PAD_Y)
            }
        })
        .collect();

    // Build column-index lookup.
    let idx_of: IndexMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();

    // Precompute label widths for all non-self messages.
    let msg_label_widths: Vec<(usize, usize, f64)> = seq
        .items
        .iter()
        .filter_map(|item| {
            let SequenceItem::Message(m) = item else {
                return None;
            };
            let fi = *idx_of.get(m.from.as_str())?;
            let ti = *idx_of.get(m.to.as_str())?;
            if fi == ti {
                return None; // self-message, handled separately
            }
            let lw = m
                .label
                .as_deref()
                .map(|l| kozue_text::measure(l, FONT_SIZE * 0.85).0)
                .unwrap_or(0.0);
            Some((fi.min(ti), fi.max(ti), lw))
        })
        .collect();

    // Note box width for a given text.
    let note_box_width = |text: &str| kozue_text::measure(text, FONT_SIZE).0 + 2.0 * NOTE_PAD_X;

    // Span widths contributed by `Over` notes covering more than one column.
    // These join `msg_label_widths` so the shared gap-expansion + multi-gap
    // fixup keeps a span note from overflowing its columns.
    let mut span_widths: Vec<(usize, usize, f64)> = msg_label_widths.clone();
    // Extra right-overhang past a column contributed by notes anchored to it
    // (`Over` single target and `RightOf`). Combined with `self_msg_overhang`.
    let mut note_right_overhang: Vec<f64> = vec![0.0; n];
    // Extra left-overhang past a column contributed by `LeftOf` notes: the box
    // sits to the left of the lifeline and must clear the previous column.
    let mut note_left_overhang: Vec<f64> = vec![0.0; n];
    for item in &seq.items {
        let SequenceItem::Note(note) = item else {
            continue;
        };
        let nw = note_box_width(&note.text);
        let cols: Vec<usize> = note
            .targets
            .iter()
            .filter_map(|t| idx_of.get(t.as_str()).copied())
            .collect();
        if cols.is_empty() {
            continue;
        }
        match note.position {
            NotePosition::Over => {
                let lo = *cols.iter().min().unwrap();
                let hi = *cols.iter().max().unwrap();
                if lo == hi {
                    // Single-column over note: symmetric half-width overhang.
                    let half = nw / 2.0;
                    if half > note_right_overhang[lo] {
                        note_right_overhang[lo] = half;
                    }
                    if half > note_left_overhang[lo] {
                        note_left_overhang[lo] = half;
                    }
                } else {
                    span_widths.push((lo, hi, nw));
                }
            }
            NotePosition::RightOf => {
                let c = cols[0];
                let overhang = NOTE_GAP + nw;
                if overhang > note_right_overhang[c] {
                    note_right_overhang[c] = overhang;
                }
            }
            NotePosition::LeftOf => {
                let c = cols[0];
                let overhang = NOTE_GAP + nw;
                if overhang > note_left_overhang[c] {
                    note_left_overhang[c] = overhang;
                }
            }
            _ => {}
        }
    }

    // Reference frames contribute to column widths like an `Over` note spanning
    // their targeted participants: a multi-column reference joins `span_widths`,
    // a single-column reference adds a symmetric half-width overhang.
    for item in &seq.items {
        let SequenceItem::Reference(reference) = item else {
            continue;
        };
        let nw = note_box_width(&reference.text);
        let cols: Vec<usize> = reference
            .targets
            .iter()
            .filter_map(|t| idx_of.get(t.as_str()).copied())
            .collect();
        if cols.is_empty() {
            continue;
        }
        let lo = *cols.iter().min().unwrap();
        let hi = *cols.iter().max().unwrap();
        if lo == hi {
            let half = nw / 2.0;
            if half > note_right_overhang[lo] {
                note_right_overhang[lo] = half;
            }
            if half > note_left_overhang[lo] {
                note_left_overhang[lo] = half;
            }
        } else {
            span_widths.push((lo, hi, nw));
        }
    }

    // Precompute self-message label widths per column.
    // For column i with a self-message, the label is placed at x1+4 = col_x[i]+SELF_MSG_WIDTH+4
    // with TextAlign::Start. The right edge is: col_x[i] + SELF_MSG_WIDTH + 4 + label_width.
    // We record the maximum overhang past col_x[i] across all self-messages on that column.
    let mut self_msg_overhang: Vec<f64> = vec![0.0; n];
    for item in &seq.items {
        let SequenceItem::Message(m) = item else {
            continue;
        };
        let fi = match idx_of.get(m.from.as_str()) {
            Some(&v) => v,
            None => continue,
        };
        let ti = match idx_of.get(m.to.as_str()) {
            Some(&v) => v,
            None => continue,
        };
        if fi != ti {
            continue;
        }
        // Self-message on column fi.
        let label_width = m
            .label
            .as_deref()
            .map(|l| kozue_text::measure(l, FONT_SIZE * 0.85).0)
            .unwrap_or(0.0);
        // Overhang from col_x[fi]: SELF_MSG_WIDTH + 4.0 (label x offset) + label_width
        let overhang = SELF_MSG_WIDTH + 4.0 + label_width;
        if overhang > self_msg_overhang[fi] {
            self_msg_overhang[fi] = overhang;
        }
    }

    // Place column centers left to right.
    let mut col_x = vec![0.0f64; n];
    if n > 0 {
        col_x[0] = header_sizes[0].0 / 2.0;
        for i in 1..n {
            let half_prev = header_sizes[i - 1].0 / 2.0;
            let half_cur = header_sizes[i].0 / 2.0;
            let base_gap = half_prev + MIN_COL_GAP + half_cur;

            // Check messages/notes spanning exactly the adjacent pair [i-1, i].
            let label_gap = span_widths
                .iter()
                .filter(|&&(a, b, _)| a == i - 1 && b == i)
                .map(|&(_, _, lw)| lw + MSG_LABEL_PAD + half_prev + half_cur)
                .fold(0.0f64, f64::max);

            // Right-side overhang from column i-1 (self-message labels and
            // right/over notes) must clear the left edge of column i's header.
            // So: col_x[i] - col_x[i-1] >= overhang + half_cur + MSG_LABEL_PAD
            let right_overhang = self_msg_overhang[i - 1].max(note_right_overhang[i - 1]);
            let self_gap = if right_overhang > 0.0 {
                right_overhang + half_cur + MSG_LABEL_PAD
            } else {
                0.0
            };

            // Left-side overhang of column i (left/over notes) must clear the
            // right edge of column i-1's header.
            let note_gap = if note_left_overhang[i] > 0.0 {
                half_prev + MSG_LABEL_PAD + note_left_overhang[i]
            } else {
                0.0
            };

            col_x[i] = col_x[i - 1] + base_gap.max(label_gap).max(self_gap).max(note_gap);
        }

        // Fixup pass: ensure messages spanning more than one gap have enough total span.
        let mut changed = true;
        while changed {
            changed = false;
            for &(a, b, lw) in &span_widths {
                if b > a + 1 {
                    let needed = col_x[a] + lw + MSG_LABEL_PAD;
                    if needed > col_x[b] {
                        let extra = (needed - col_x[b]) / (b - a) as f64;
                        for (offset, col) in col_x[(a + 1)..=b].iter_mut().enumerate() {
                            *col += extra * (offset + 1) as f64;
                        }
                        changed = true;
                    }
                }
            }
        }
    }

    // Max header height (all headers same height for visual consistency).
    let header_height = header_sizes.iter().map(|&(_, h)| h).fold(0.0f64, f64::max);

    let msg_count = seq.items.len();
    let diagram_bottom = MSG_START_Y + msg_count as f64 * MSG_ROW_HEIGHT + LIFELINE_EXTRA;

    let mut items: Vec<SceneItem> = Vec::new();
    let mut sem_participants: Vec<semantic::ParticipantLayout> = Vec::new();
    let mut sem_items: Vec<SequenceItemLayout> = Vec::new();

    // Draw participant headers and lifelines.
    for (i, id) in ids.iter().enumerate() {
        let p = &seq.participants[*id];
        let label = &p.label;
        let (hw, _hh) = header_sizes[i];
        let cx = col_x[i];
        let lifeline_top = HEADER_TOP + header_height;

        // Header rect (use uniform header_height for all boxes).
        items.push(SceneItem::Rect(Rect {
            x: cx - hw / 2.0,
            y: HEADER_TOP,
            width: hw,
            height: header_height,
            rx: HEADER_RX,
        }));

        let (tw, th) = kozue_text::measure(label, FONT_SIZE);

        // For non-Default participants: emit stereotype line above label.
        if let Some(st) = stereotype_label(&p.kind) {
            let (stw, sth) = kozue_text::measure(st, STEREOTYPE_FONT_SIZE);
            // Compute vertical positions: center the {stereotype + gap + label} block in the header.
            let block_height = sth + STEREOTYPE_GAP + th;
            let block_top = HEADER_TOP + (header_height - block_height) / 2.0;
            let st_y = block_top + STEREOTYPE_FONT_SIZE * 0.85; // baseline of stereotype text
            items.push(SceneItem::Text(Text {
                x: cx,
                y: st_y,
                size: STEREOTYPE_FONT_SIZE,
                align: TextAlign::Middle,
                content: st.to_string(),
                text_width: stw,
                text_height: sth,
            }));
            // Label sits below the stereotype line.
            let label_y = block_top + sth + STEREOTYPE_GAP + FONT_SIZE * 0.35;
            items.push(SceneItem::Text(Text {
                x: cx,
                y: label_y,
                size: FONT_SIZE,
                align: TextAlign::Middle,
                content: label.clone(),
                text_width: tw,
                text_height: th,
            }));
        } else {
            // Default: original positioning (byte-identical to pre-V9 output).
            items.push(SceneItem::Text(Text {
                x: cx,
                y: HEADER_TOP + header_height / 2.0 + FONT_SIZE * 0.35,
                size: FONT_SIZE,
                align: TextAlign::Middle,
                content: label.clone(),
                text_width: tw,
                text_height: th,
            }));
        }

        // Lifeline: dashed vertical line from bottom of header to diagram_bottom.
        items.push(SceneItem::Path(Path {
            points: vec![(cx, lifeline_top), (cx, diagram_bottom)],
            filled: false,
            stroke: StrokeStyle::Dashed,
            weight: StrokeWeight::Normal,
        }));

        sem_participants.push(semantic::ParticipantLayout {
            id: (*id).clone(),
            label: label.clone(),
            kind: p.kind.clone(),
            header_rect: Rect {
                x: cx - hw / 2.0,
                y: HEADER_TOP,
                width: hw,
                height: header_height,
                rx: HEADER_RX,
            },
            lifeline_x: cx,
            lifeline_y0: lifeline_top,
            lifeline_y1: diagram_bottom,
        });
    }

    // Per-participant stack of y_starts for open activation bars.
    let mut active_stacks: HashMap<&str, Vec<f64>> = HashMap::new();
    let mut sem_bars: Vec<semantic::ActivationBarLayout> = Vec::new();

    // Record the index at which to splice activation bar rects.  All bar rects
    // must appear *after* lifelines (so they are drawn on top of the dashed
    // lines) but *before* messages and notes (so messages are painted on top of
    // bars).  We also sort bars by depth ascending (outer first, inner last) so
    // the inner bar's filled rectangle occludes the outer one — painter's order.
    let bars_insert_idx = items.len();

    // Temporary storage for bar rects collected during deactivation.
    // Each entry is (depth, Rect) so we can sort before splicing.
    let mut pending_bar_rects: Vec<(u32, Rect)> = Vec::new();

    // Draw body items (messages and notes) in declaration order.
    for (row, item) in seq.items.iter().enumerate() {
        let msg = match item {
            SequenceItem::Message(msg) => msg,
            SequenceItem::Note(note) => {
                draw_note(
                    &mut items,
                    &mut sem_items,
                    note,
                    row,
                    &col_x,
                    &idx_of,
                    &note_box_width,
                );
                continue;
            }
            SequenceItem::Divider(divider) => {
                draw_divider(&mut items, &mut sem_items, divider, row, &col_x);
                continue;
            }
            SequenceItem::Delay(delay) => {
                draw_delay(&mut items, &mut sem_items, delay, row, &col_x);
                continue;
            }
            SequenceItem::Reference(reference) => {
                draw_reference(&mut items, &mut sem_items, reference, row, &col_x, &idx_of);
                continue;
            }
            SequenceItem::Activate(act) => {
                let y = MSG_START_Y + row as f64 * MSG_ROW_HEIGHT;
                let ci = idx_of[act.participant.as_str()];
                let cx = col_x[ci];
                let stack = active_stacks.entry(act.participant.as_str()).or_default();
                let depth = stack.len() as u32;
                stack.push(y);
                sem_items.push(SequenceItemLayout::Activation(
                    semantic::ActivationMarkerLayout {
                        index: row,
                        participant: act.participant.clone(),
                        x: cx,
                        y,
                        is_start: true,
                        depth,
                    },
                ));
                continue;
            }
            SequenceItem::Deactivate(act) => {
                let y = MSG_START_Y + row as f64 * MSG_ROW_HEIGHT;
                let ci = idx_of[act.participant.as_str()];
                let cx = col_x[ci];
                let stack = active_stacks.get_mut(act.participant.as_str());
                let y0 = match stack.and_then(|s| s.pop()) {
                    Some(v) => v,
                    None => {
                        return Err(crate::LayoutError {
                            message: format!(
                                "deactivate {} has no matching activate",
                                act.participant
                            ),
                        });
                    }
                };
                let depth = active_stacks
                    .get(act.participant.as_str())
                    .map(|s| s.len() as u32)
                    .unwrap_or(0);
                let bar_x = cx - BAR_WIDTH / 2.0 + depth as f64 * BAR_NEST_OFFSET;
                let bar_rect = Rect {
                    x: bar_x,
                    y: y0,
                    width: BAR_WIDTH,
                    height: y - y0,
                    rx: 0.0,
                };
                sem_bars.push(semantic::ActivationBarLayout {
                    participant: act.participant.clone(),
                    rect: bar_rect,
                    depth,
                });
                // Collect bar rect for deferred, depth-ordered insertion.
                // (Actual items.push happens after the loop — see below.)
                pending_bar_rects.push((depth, sem_bars.last().unwrap().rect.clone()));
                sem_items.push(SequenceItemLayout::Activation(
                    semantic::ActivationMarkerLayout {
                        index: row,
                        participant: act.participant.clone(),
                        x: cx,
                        y,
                        is_start: false,
                        depth,
                    },
                ));
                continue;
            }
            _ => continue,
        };
        let y = MSG_START_Y + row as f64 * MSG_ROW_HEIGHT;

        let fi = idx_of[msg.from.as_str()];
        let ti = idx_of[msg.to.as_str()];

        if fi == ti {
            // Self-message: U-shape (right, down, left with arrowhead).
            let cx = col_x[fi];
            let depth_fi = active_stacks
                .get(msg.from.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            let self_offset = if depth_fi > 0 {
                BAR_WIDTH / 2.0 + (depth_fi - 1) as f64 * BAR_NEST_OFFSET
            } else {
                0.0
            };
            let x0 = cx + self_offset;
            let x1 = cx + self_offset + SELF_MSG_WIDTH;
            let y0 = y;
            let y1 = y + SELF_MSG_HEIGHT;

            // Retract the shaft where a filled head/tail covers the line end.
            // For the default (head=Filled, tail=None) this reproduces the
            // pre-V10 geometry byte-for-byte.
            let head_r = shaft_retraction(msg.head);
            let tail_r = shaft_retraction(msg.tail);

            let stroke = crate::line_style_to_stroke(msg.line);
            items.push(SceneItem::Path(Path {
                points: vec![(x0 + tail_r, y0), (x1, y0), (x1, y1), (x0 + head_r, y1)],
                filled: false,
                stroke,
                weight: StrokeWeight::Normal,
            }));

            // Head glyph pointing left at (x0, y1); tail glyph pointing left
            // at the loop start (x0, y0).
            push_arrow_glyph(&mut items, (x0, y1), -1.0, msg.head);
            push_arrow_glyph(&mut items, (x0, y0), -1.0, msg.tail);

            // Label to the right of the fold.
            let label_anchor = if let Some(label) = &msg.label {
                let (tw, th) = kozue_text::measure(label, FONT_SIZE * 0.85);
                items.push(SceneItem::Text(Text {
                    x: x1 + 4.0,
                    y: y0 + FONT_SIZE * 0.85 * 0.35,
                    size: FONT_SIZE * 0.85,
                    align: TextAlign::Start,
                    content: label.clone(),
                    text_width: tw,
                    text_height: th,
                }));
                // Semantic anchor: center of label text (Start-aligned → x is the left edge)
                Some(semantic::Point::new(
                    x1 + 4.0 + tw / 2.0,
                    y0 + FONT_SIZE * 0.85 * 0.35,
                ))
            } else {
                None
            };

            // Route: the self-loop polyline (same points as the path above, tip is the end)
            let route = vec![
                semantic::Point::new(x0, y0),
                semantic::Point::new(x1, y0),
                semantic::Point::new(x1, y1),
                semantic::Point::new(x0, y1),
            ];
            sem_items.push(SequenceItemLayout::Message(semantic::MessageLayout {
                index: row,
                from: msg.from.clone(),
                to: msg.to.clone(),
                route,
                line: msg.line,
                head: msg.head,
                tail: msg.tail,
                label: msg.label.clone(),
                label_anchor,
            }));
        } else {
            // Horizontal arrow from fi to ti.
            let raw_from = col_x[fi];
            let raw_to = col_x[ti];
            let going_right = raw_to > raw_from;
            let depth_from = active_stacks
                .get(msg.from.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            let depth_to = active_stacks
                .get(msg.to.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            // Endpoint clings to the edge of the open activation bar.
            // Bar geometry: bar_x = cx - BAR_WIDTH/2 + depth*BAR_NEST_OFFSET (right-shifts
            // with each nesting level). The bar's right edge is bar_x + BAR_WIDTH =
            // cx + BAR_WIDTH/2 + (depth-1)*BAR_NEST_OFFSET.  The bar's left edge is
            // bar_x = cx - BAR_WIDTH/2 + (depth-1)*BAR_NEST_OFFSET.
            // The nest term (depth-1)*BAR_NEST_OFFSET is always *added* (bar shifts right);
            // only the BAR_WIDTH/2 half-width flips sign depending on which edge we pick.
            let x_from = if depth_from > 0 {
                let nest = (depth_from - 1) as f64 * BAR_NEST_OFFSET;
                if going_right {
                    // Message leaves from the right edge of the bar.
                    raw_from + BAR_WIDTH / 2.0 + nest
                } else {
                    // Message leaves from the left edge of the bar.
                    raw_from - BAR_WIDTH / 2.0 + nest
                }
            } else {
                raw_from
            };
            let x_to = if depth_to > 0 {
                let nest = (depth_to - 1) as f64 * BAR_NEST_OFFSET;
                if going_right {
                    // Message arrives at the left edge of the bar.
                    raw_to - BAR_WIDTH / 2.0 + nest
                } else {
                    // Message arrives at the right edge of the bar.
                    raw_to + BAR_WIDTH / 2.0 + nest
                }
            } else {
                raw_to
            };
            let ux = if going_right { 1.0 } else { -1.0 };

            // Retract the shaft where a filled head/tail covers the line end.
            // For the default (head=Filled, tail=None) this reproduces the
            // pre-V10 geometry byte-for-byte.
            let head_r = shaft_retraction(msg.head);
            let tail_r = shaft_retraction(msg.tail);

            let stroke = crate::line_style_to_stroke(msg.line);
            let line_start_x = x_from + ux * tail_r;
            let line_end_x = x_to - ux * head_r;

            items.push(SceneItem::Path(Path {
                points: vec![(line_start_x, y), (line_end_x, y)],
                filled: false,
                stroke,
                weight: StrokeWeight::Normal,
            }));

            // Head glyph at the target end, tail glyph at the source end.
            push_arrow_glyph(&mut items, (x_to, y), ux, msg.head);
            push_arrow_glyph(&mut items, (x_from, y), -ux, msg.tail);

            // Label above center of line.
            let label_anchor = if let Some(label) = &msg.label {
                let mx = (x_from + x_to) / 2.0;
                let (tw, th) = kozue_text::measure(label, FONT_SIZE * 0.85);
                items.push(SceneItem::Text(Text {
                    x: mx,
                    y: y - 4.0,
                    size: FONT_SIZE * 0.85,
                    align: TextAlign::Middle,
                    content: label.clone(),
                    text_width: tw,
                    text_height: th,
                }));
                Some(semantic::Point::new(mx, y - 4.0))
            } else {
                None
            };

            // Route: source → tip of arrow.
            let route = vec![
                semantic::Point::new(x_from, y),
                semantic::Point::new(x_to, y),
            ];
            sem_items.push(SequenceItemLayout::Message(semantic::MessageLayout {
                index: row,
                from: msg.from.clone(),
                to: msg.to.clone(),
                route,
                line: msg.line,
                head: msg.head,
                tail: msg.tail,
                label: msg.label.clone(),
                label_anchor,
            }));
        }
    }

    // Check for unclosed activations.
    for (pid, stack) in &active_stacks {
        if !stack.is_empty() {
            return Err(crate::LayoutError {
                message: format!("participant {} has unclosed activate", pid),
            });
        }
    }

    // Splice collected bar rects into the scene at `bars_insert_idx` (after all
    // lifelines but before all messages/notes).  Sort depth ascending so outer
    // bars (smaller depth) are painted first and inner bars appear on top.
    pending_bar_rects.sort_by_key(|&(depth, _)| depth);
    let bar_scene_items: Vec<SceneItem> = pending_bar_rects
        .into_iter()
        .map(|(_, rect)| SceneItem::Rect(rect))
        .collect();
    items.splice(bars_insert_idx..bars_insert_idx, bar_scene_items);

    // Also sort sem_bars depth ascending so backend renderers (drawio/excalidraw/…)
    // draw outer bars before inner bars, matching the scene painter's order.
    sem_bars.sort_by_key(|b| b.depth);

    // Normalize bounds.
    let (min_x, min_y, max_x, max_y) = bounds::scene_bounds(&items);
    bounds::translate(&mut items, -min_x, -min_y);

    // Apply the same translation to all semantic coordinates.
    for p in &mut sem_participants {
        p.header_rect.x -= min_x;
        p.header_rect.y -= min_y;
        p.lifeline_x -= min_x;
        p.lifeline_y0 -= min_y;
        p.lifeline_y1 -= min_y;
    }
    for item in &mut sem_items {
        match item {
            SequenceItemLayout::Message(m) => {
                for pt in &mut m.route {
                    pt.x -= min_x;
                    pt.y -= min_y;
                }
                if let Some(la) = &mut m.label_anchor {
                    la.x -= min_x;
                    la.y -= min_y;
                }
            }
            SequenceItemLayout::Note(note) => {
                note.rect.x -= min_x;
                note.rect.y -= min_y;
                note.text_anchor.x -= min_x;
                note.text_anchor.y -= min_y;
            }
            SequenceItemLayout::Divider(divider) => {
                divider.rect.x -= min_x;
                divider.rect.y -= min_y;
                divider.text_anchor.x -= min_x;
                divider.text_anchor.y -= min_y;
            }
            SequenceItemLayout::Delay(delay) => {
                delay.rect.x -= min_x;
                delay.rect.y -= min_y;
                if let Some(anchor) = &mut delay.text_anchor {
                    anchor.x -= min_x;
                    anchor.y -= min_y;
                }
            }
            SequenceItemLayout::Reference(reference) => {
                reference.rect.x -= min_x;
                reference.rect.y -= min_y;
                reference.text_anchor.x -= min_x;
                reference.text_anchor.y -= min_y;
            }
            SequenceItemLayout::Activation(marker) => {
                marker.x -= min_x;
                marker.y -= min_y;
            }
        }
    }

    // Apply the same translation to bars.
    for bar in &mut sem_bars {
        bar.rect.x -= min_x;
        bar.rect.y -= min_y;
    }

    let scene = Scene {
        width: max_x - min_x,
        height: max_y - min_y,
        items,
    };
    let sem = crate::semantic::SemanticLayout::Sequence(semantic::SequenceLayout {
        participants: sem_participants,
        items: sem_items,
        bars: sem_bars,
    });

    Ok(crate::LayoutOutput {
        scene,
        semantic: sem,
    })
}
