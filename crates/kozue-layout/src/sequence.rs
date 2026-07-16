//! Deterministic arithmetic layout for sequence diagrams.

use indexmap::IndexMap;
use kozue_ir::{
    ElementId, LineStyle, Path, Rect, Scene, SceneItem, SequenceDiagram, SequenceItem, Text,
    TextAlign,
};

use crate::bounds;
use crate::semantic;

const FONT_SIZE: f64 = 16.0;
const PAD_X: f64 = 20.0;
const PAD_Y: f64 = 10.0;
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

pub(crate) fn layout_sequence_full(
    seq: &SequenceDiagram,
) -> Result<crate::LayoutOutput, crate::LayoutError> {
    for item in &seq.items {
        let SequenceItem::Message(message) = item else {
            return Err(crate::LayoutError {
                message: "unsupported future sequence item".to_string(),
            });
        };
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
        crate::validate_arrow(message.arrow)?;
    }
    // Collect participant IDs in insertion order.
    let ids: Vec<&ElementId> = seq.participants.keys().collect();
    let n = ids.len();

    // Measure each header box.
    let header_sizes: Vec<(f64, f64)> = ids
        .iter()
        .map(|id| {
            let label = &seq.participants[*id].label;
            let (tw, th) = kozue_text::measure(label, FONT_SIZE);
            (tw + 2.0 * PAD_X, th + 2.0 * PAD_Y)
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

            // Check messages spanning exactly the adjacent pair [i-1, i].
            let label_gap = msg_label_widths
                .iter()
                .filter(|&&(a, b, _)| a == i - 1 && b == i)
                .map(|&(_, _, lw)| lw + MSG_LABEL_PAD + half_prev + half_cur)
                .fold(0.0f64, f64::max);

            // Self-message overhang from column i-1: the label extends to
            // col_x[i-1] + self_msg_overhang[i-1], which must not overlap the
            // left edge of column i's header (col_x[i] - half_cur).
            // So: col_x[i] - col_x[i-1] >= self_msg_overhang[i-1] + half_cur + MSG_LABEL_PAD
            let self_gap = if self_msg_overhang[i - 1] > 0.0 {
                self_msg_overhang[i - 1] + half_cur + MSG_LABEL_PAD
            } else {
                0.0
            };

            col_x[i] = col_x[i - 1] + base_gap.max(label_gap).max(self_gap);
        }

        // Fixup pass: ensure messages spanning more than one gap have enough total span.
        let mut changed = true;
        while changed {
            changed = false;
            for &(a, b, lw) in &msg_label_widths {
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
    let mut sem_messages: Vec<semantic::MessageLayout> = Vec::new();

    // Draw participant headers and lifelines.
    for (i, id) in ids.iter().enumerate() {
        let label = &seq.participants[*id].label;
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

        // Header label.
        let (tw, th) = kozue_text::measure(label, FONT_SIZE);
        items.push(SceneItem::Text(Text {
            x: cx,
            y: HEADER_TOP + header_height / 2.0 + FONT_SIZE * 0.35,
            size: FONT_SIZE,
            align: TextAlign::Middle,
            content: label.clone(),
            text_width: tw,
            text_height: th,
        }));

        // Lifeline: dashed vertical line from bottom of header to diagram_bottom.
        items.push(SceneItem::Path(Path {
            points: vec![(cx, lifeline_top), (cx, diagram_bottom)],
            filled: false,
            dashed: true,
        }));

        sem_participants.push(semantic::ParticipantLayout {
            id: (*id).clone(),
            label: label.clone(),
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

    // Draw messages.
    for (row, item) in seq.items.iter().enumerate() {
        let SequenceItem::Message(msg) = item else {
            continue;
        };
        let y = MSG_START_Y + row as f64 * MSG_ROW_HEIGHT;

        let fi = idx_of[msg.from.as_str()];
        let ti = idx_of[msg.to.as_str()];

        if fi == ti {
            // Self-message: コの字型 (right, down, left with arrowhead).
            let cx = col_x[fi];
            let x0 = cx;
            let x1 = cx + SELF_MSG_WIDTH;
            let y0 = y;
            let y1 = y + SELF_MSG_HEIGHT;

            let dashed = matches!(msg.line, LineStyle::Dashed);
            items.push(SceneItem::Path(Path {
                points: vec![(x0, y0), (x1, y0), (x1, y1), (x0 + ARROW_LEN, y1)],
                filled: false,
                dashed,
            }));

            // Arrowhead pointing left at (x0, y1).
            let tip = (x0, y1);
            let left = (x0 + ARROW_LEN, y1 - ARROW_HALF_W);
            let right = (x0 + ARROW_LEN, y1 + ARROW_HALF_W);
            items.push(SceneItem::Path(Path {
                points: vec![tip, left, right],
                filled: true,
                dashed: false,
            }));

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
            sem_messages.push(semantic::MessageLayout {
                index: row,
                from: msg.from.clone(),
                to: msg.to.clone(),
                route,
                line: msg.line,
                arrow: msg.arrow,
                label: msg.label.clone(),
                label_anchor,
            });
        } else {
            // Horizontal arrow from fi to ti.
            let x_from = col_x[fi];
            let x_to = col_x[ti];
            let going_right = x_to > x_from;
            let ux = if going_right { 1.0 } else { -1.0 };

            let dashed = matches!(msg.line, LineStyle::Dashed);
            let line_end_x = x_to - ux * ARROW_LEN;

            items.push(SceneItem::Path(Path {
                points: vec![(x_from, y), (line_end_x, y)],
                filled: false,
                dashed,
            }));

            // Arrowhead.
            let tip = (x_to, y);
            let base_left = (x_to - ux * ARROW_LEN, y - ARROW_HALF_W);
            let base_right = (x_to - ux * ARROW_LEN, y + ARROW_HALF_W);
            items.push(SceneItem::Path(Path {
                points: vec![tip, base_left, base_right],
                filled: true,
                dashed: false,
            }));

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
            sem_messages.push(semantic::MessageLayout {
                index: row,
                from: msg.from.clone(),
                to: msg.to.clone(),
                route,
                line: msg.line,
                arrow: msg.arrow,
                label: msg.label.clone(),
                label_anchor,
            });
        }
    }

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
    for m in &mut sem_messages {
        for pt in &mut m.route {
            pt.x -= min_x;
            pt.y -= min_y;
        }
        if let Some(la) = &mut m.label_anchor {
            la.x -= min_x;
            la.y -= min_y;
        }
    }

    let scene = Scene {
        width: max_x - min_x,
        height: max_y - min_y,
        items,
    };
    let sem = crate::semantic::SemanticLayout::Sequence(semantic::SequenceLayout {
        participants: sem_participants,
        messages: sem_messages,
    });

    Ok(crate::LayoutOutput {
        scene,
        semantic: sem,
    })
}
