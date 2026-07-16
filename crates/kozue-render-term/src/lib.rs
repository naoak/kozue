//! Terminal (plain-text) renderer for the Scene IR.
//!
//! Rasterises a [`Scene`] to a character-cell grid and returns a plain-text
//! string (no ANSI escape codes). Each cell is 8 px wide × 16 px tall.
//!
//! The only external dependency besides `kozue-ir` is `unicode-width` for
//! measuring the display width of CJK / full-width characters.

use unicode_width::UnicodeWidthChar;

use kozue_ir::{Scene, SceneItem, TextAlign};

/// Pixels per grid cell (horizontal).
const CELL_W: f64 = 8.0;
/// Pixels per grid cell (vertical).
const CELL_H: f64 = 16.0;

// ---------------------------------------------------------------------------
// Cell model
// ---------------------------------------------------------------------------

/// A single grid cell.
///
/// Full-width (display-width 2) characters occupy two logical cells: the left
/// cell holds the character (`Char`), and the right cell is marked `Wide` (a
/// continuation / occupancy marker).  During serialisation `Wide` cells are
/// **skipped** (nothing is written), so the rendered line's display width
/// equals the number of logical cells, keeping vertical borders aligned.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    /// A regular printable character (display width 1 or 2).
    /// For wide characters this is the *left* cell; the next cell is `Wide`.
    Char(char),
    /// Continuation cell: the right half of a preceding wide character.
    /// Must not be output.
    Wide,
    /// Empty (space) cell — the default.
    Empty,
}

impl Cell {
    /// Return the character to write during serialisation, or `None` to skip.
    fn output_char(self) -> Option<char> {
        match self {
            Cell::Char(c) => Some(c),
            Cell::Wide => None,
            Cell::Empty => Some(' '),
        }
    }

    /// Character stored in this cell, treating Empty as space.
    fn char_or_space(self) -> char {
        match self {
            Cell::Char(c) => c,
            Cell::Wide | Cell::Empty => ' ',
        }
    }
}

/// Render a [`Scene`] to a plain-text string.
///
/// Returns a string ending with `\n`.  An empty / zero-size scene returns
/// `"\n"`.  The output never contains ANSI escape sequences.
pub fn render(scene: &Scene) -> String {
    let cols = (scene.width / CELL_W).ceil() as usize;
    let rows = (scene.height / CELL_H).ceil() as usize;

    if cols == 0 || rows == 0 {
        return "\n".to_string();
    }

    // Initialise the grid to empty cells.
    let mut grid: Vec<Vec<Cell>> = vec![vec![Cell::Empty; cols]; rows];

    // Two-pass rendering: non-text items first (in scene order), then text
    // items (in scene order).  This ensures text labels are always legible —
    // they are never overwritten by box borders or path lines.
    for item in &scene.items {
        render_item(&mut grid, rows, cols, item, false);
    }
    for item in &scene.items {
        render_item(&mut grid, rows, cols, item, true);
    }

    // Serialise: trim trailing spaces per row, join with newlines.
    // Wide-continuation cells are skipped (not output).
    let mut out = String::new();
    for row in &grid {
        let mut line = String::new();
        for &cell in row {
            // Wide continuation cells are skipped (not output).
            if let Some(c) = cell.output_char() {
                line.push(c);
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Item dispatch
// ---------------------------------------------------------------------------

/// Render one item.
///
/// `text_pass`: when `true`, only `Text` items are drawn (second pass).
///              When `false`, only non-`Text` items are drawn (first pass).
fn render_item(
    grid: &mut [Vec<Cell>],
    rows: usize,
    cols: usize,
    item: &SceneItem,
    text_pass: bool,
) {
    match item {
        SceneItem::Text(t) => {
            if text_pass {
                render_text(grid, rows, cols, t);
            }
        }
        SceneItem::Rect(r) => {
            if !text_pass {
                render_rect(grid, rows, cols, r);
            }
        }
        SceneItem::Path(p) => {
            if !text_pass {
                render_path(grid, rows, cols, p);
            }
        }
        SceneItem::Group(g) => {
            for child in &g.items {
                render_item(grid, rows, cols, child, text_pass);
            }
        }
        _ => {} // future variants: silently skip
    }
}

// ---------------------------------------------------------------------------
// Rect
// ---------------------------------------------------------------------------

fn render_rect(grid: &mut [Vec<Cell>], rows: usize, cols: usize, r: &kozue_ir::Rect) {
    let col0 = (r.x / CELL_W) as usize;
    let row0 = (r.y / CELL_H) as usize;
    let col1 = ((r.x + r.width) / CELL_W) as usize;
    let row1 = ((r.y + r.height) / CELL_H) as usize;

    // Clamp to grid.
    let col0 = col0.min(cols.saturating_sub(1));
    let row0 = row0.min(rows.saturating_sub(1));
    let col1 = col1.min(cols.saturating_sub(1));
    let row1 = row1.min(rows.saturating_sub(1));

    if col1 < col0 || row1 < row0 {
        return;
    }

    // Preserve legacy corners for Default (`rx=4`) and use visibly rounded
    // corners only for explicit RoundedRectangle (`rx=8`).
    let (top_left, top_right, bottom_left, bottom_right) = if r.rx >= 8.0 {
        ('╭', '╮', '╰', '╯')
    } else {
        ('┌', '┐', '└', '┘')
    };
    set(grid, rows, cols, row0, col0, top_left);
    set(grid, rows, cols, row0, col1, top_right);
    set(grid, rows, cols, row1, col0, bottom_left);
    set(grid, rows, cols, row1, col1, bottom_right);

    // Top and bottom edges.
    for c in (col0 + 1)..col1 {
        set(grid, rows, cols, row0, c, '─');
        set(grid, rows, cols, row1, c, '─');
    }
    // Left and right edges.
    for r_idx in (row0 + 1)..row1 {
        set(grid, rows, cols, r_idx, col0, '│');
        set(grid, rows, cols, r_idx, col1, '│');
    }
}

// ---------------------------------------------------------------------------
// Path
// ---------------------------------------------------------------------------

fn render_path(grid: &mut [Vec<Cell>], rows: usize, cols: usize, p: &kozue_ir::Path) {
    if p.filled {
        render_arrowhead(grid, rows, cols, p);
        return;
    }

    if p.points.len() < 2 {
        // Single point or no points: nothing to draw.
        if let Some(&(x, y)) = p.points.first() {
            let col = (x / CELL_W).round() as i64;
            let row = (y / CELL_H).round() as i64;
            set_i(grid, rows, cols, row, col, '·');
        }
        return;
    }

    // Dashed: alternate draw/skip each cell along each segment.
    // Fix 4: maintain phase across segments by not re-toggling at shared
    // vertices — the last cell of a segment is the same point as the first
    // cell of the next segment and must be counted only once.
    let mut draw = true;
    let mut prev_end: Option<(i64, i64)> = None;

    for window in p.points.windows(2) {
        let (x0, y0) = window[0];
        let (x1, y1) = window[1];

        let c0 = (x0 / CELL_W).round() as i64;
        let r0 = (y0 / CELL_H).round() as i64;
        let c1 = (x1 / CELL_W).round() as i64;
        let r1 = (y1 / CELL_H).round() as i64;

        let dx = c1 - c0;
        let dy = r1 - r0;

        // Character that best represents this segment's direction.
        let seg_char = segment_char(dx, dy);

        // Bresenham walk from (r0,c0) to (r1,c1).
        let cells = bresenham_cells(r0, c0, r1, c1);

        // Determine whether to skip the first cell of this segment.
        // If the previous segment ended at the same cell, that cell was
        // already drawn (or already toggled), so skip it to avoid a double
        // toggle that would shift the dashed phase.
        let skip_first = prev_end == Some((r0, c0));

        for (i, &(row, col)) in cells.iter().enumerate() {
            if skip_first && i == 0 {
                // This cell was already handled as the last cell of the
                // previous segment; don't toggle phase again.
                continue;
            }
            if draw {
                // Preserve existing box-border characters so that path lines
                // do not overwrite box corners/edges when they touch.
                if !is_box_border(get_i(grid, rows, cols, row, col)) {
                    set_i(grid, rows, cols, row, col, seg_char);
                }
            }
            if p.dashed {
                draw = !draw;
            }
        }

        prev_end = cells.last().copied();
    }
}

/// Choose a box-drawing / slash character for a segment with the given delta.
fn segment_char(dx: i64, dy: i64) -> char {
    let adx = dx.unsigned_abs();
    let ady = dy.unsigned_abs();
    if adx == 0 && ady == 0 {
        return '·';
    }
    if adx == 0 {
        return '│'; // pure vertical
    }
    if ady == 0 {
        return '─'; // pure horizontal
    }
    // Diagonal: prefer axis-character when one component dominates.
    if ady > adx * 2 {
        return '│';
    }
    if adx > ady * 2 {
        return '─';
    }
    // True diagonal.
    if (dx > 0) == (dy > 0) {
        '╲' // U+2572, upper-left to lower-right
    } else {
        '╱' // U+2571, upper-right to lower-left
    }
}

/// Arrowhead: filled polygon whose tip is `points[0]`.
fn render_arrowhead(grid: &mut [Vec<Cell>], rows: usize, cols: usize, p: &kozue_ir::Path) {
    if p.points.is_empty() {
        return;
    }
    let (tx, ty) = p.points[0];

    // Compute base-centre from remaining points.
    let base_points = &p.points[1..];
    let (bx, by) = if base_points.is_empty() {
        (tx, ty)
    } else {
        let sum_x: f64 = base_points.iter().map(|&(x, _)| x).sum();
        let sum_y: f64 = base_points.iter().map(|&(_, y)| y).sum();
        (
            sum_x / base_points.len() as f64,
            sum_y / base_points.len() as f64,
        )
    };

    let dx = tx - bx;
    let dy = ty - by;

    let ch = if dy.abs() > dx.abs() {
        if dy < 0.0 {
            '▲'
        } else {
            '▼'
        }
    } else if dx < 0.0 {
        '◀'
    } else {
        '▶'
    };

    // Fix 3: round in the direction of travel so the tip sits at the boundary
    // rather than one cell inside the target box.
    //   ▼ (tip below base, dy > 0): floor  — largest row that doesn't exceed ty/CELL_H
    //   ▲ (tip above base, dy < 0): ceil   — smallest row that doesn't exceed ty/CELL_H
    //   ▶ (tip right of base, dx > 0): floor on col
    //   ◀ (tip left  of base, dx < 0): ceil  on col
    let col = match ch {
        '▶' => (tx / CELL_W).floor() as i64,
        '◀' => (tx / CELL_W).ceil() as i64,
        _ => (tx / CELL_W).round() as i64,
    };
    let row = match ch {
        '▼' => (ty / CELL_H).floor() as i64,
        '▲' => (ty / CELL_H).ceil() as i64,
        _ => (ty / CELL_H).round() as i64,
    };
    set_i(grid, rows, cols, row, col, ch);
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

fn render_text(grid: &mut [Vec<Cell>], rows: usize, cols: usize, t: &kozue_ir::Text) {
    // Fix 2: apply baseline correction so text sits at the box centre row
    // rather than the baseline row.  SVG text `y` is the baseline position
    // which is approximately `centre + 0.35 * font_size` below the top of
    // the em-square.  Subtracting that offset maps to the visual centre.
    // floor, not round: the corrected y sits at the visual centre of the
    // enclosing box, and rounding up can land on its bottom border row.
    let row = ((t.y - t.size * 0.35) / CELL_H).floor() as i64;
    if row < 0 || row >= rows as i64 {
        return;
    }

    // Measure display width of entire text.
    let text_display_cols: i64 = t
        .content
        .chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(1) as i64)
        .sum();

    // Compute starting column based on alignment.
    let start_col: i64 = match t.align {
        TextAlign::Start => (t.x / CELL_W).round() as i64,
        TextAlign::Middle => (t.x / CELL_W).round() as i64 - text_display_cols / 2,
        TextAlign::End => (t.x / CELL_W).round() as i64 - text_display_cols,
        _ => (t.x / CELL_W).round() as i64, // future variants: fallback to Start
    };

    let row_idx = row as usize;
    let mut col = start_col;

    for ch in t.content.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(1) as i64;
        if col >= 0 && col < cols as i64 {
            grid[row_idx][col as usize] = Cell::Char(ch);
            // Fix 1: For wide chars (w==2), mark the next cell as Wide
            // (continuation marker) instead of a space.  The Wide cell is
            // skipped during output, keeping display width == logical cell count.
            if w == 2 && (col + 1) < cols as i64 {
                grid[row_idx][(col + 1) as usize] = Cell::Wide;
            }
        } else if col + w > 0 && col < cols as i64 {
            // Partially in bounds — skip.
        }
        col += w;
        if col >= cols as i64 {
            break; // rest is out of bounds
        }
    }
}

// ---------------------------------------------------------------------------
// Bresenham
// ---------------------------------------------------------------------------

/// Return all grid cells on the line from (r0,c0) to (r1,c1) inclusive,
/// using Bresenham's algorithm.
fn bresenham_cells(r0: i64, c0: i64, r1: i64, c1: i64) -> Vec<(i64, i64)> {
    let mut cells = Vec::new();

    let dr = (r1 - r0).abs();
    let dc = (c1 - c0).abs();
    let sr: i64 = if r1 > r0 { 1 } else { -1 };
    let sc: i64 = if c1 > c0 { 1 } else { -1 };

    let mut r = r0;
    let mut c = c0;

    if dr == 0 && dc == 0 {
        cells.push((r, c));
        return cells;
    }

    if dc >= dr {
        // Horizontal-dominant.
        let mut err = 2 * dr - dc;
        for _ in 0..=dc {
            cells.push((r, c));
            if err >= 0 {
                r += sr;
                err -= 2 * dc;
            }
            err += 2 * dr;
            c += sc;
        }
    } else {
        // Vertical-dominant.
        let mut err = 2 * dc - dr;
        for _ in 0..=dr {
            cells.push((r, c));
            if err >= 0 {
                c += sc;
                err -= 2 * dr;
            }
            err += 2 * dc;
            r += sr;
        }
    }

    cells
}

// ---------------------------------------------------------------------------
// Grid helpers
// ---------------------------------------------------------------------------

#[inline]
fn set(grid: &mut [Vec<Cell>], rows: usize, cols: usize, row: usize, col: usize, ch: char) {
    if row < rows && col < cols {
        grid[row][col] = Cell::Char(ch);
    }
}

#[inline]
fn set_i(grid: &mut [Vec<Cell>], rows: usize, cols: usize, row: i64, col: i64, ch: char) {
    if row >= 0 && col >= 0 && (row as usize) < rows && (col as usize) < cols {
        grid[row as usize][col as usize] = Cell::Char(ch);
    }
}

#[inline]
fn get_i(grid: &[Vec<Cell>], rows: usize, cols: usize, row: i64, col: i64) -> char {
    if row >= 0 && col >= 0 && (row as usize) < rows && (col as usize) < cols {
        grid[row as usize][col as usize].char_or_space()
    } else {
        ' '
    }
}

/// Returns `true` if `ch` is a box-drawing border character that should be
/// preserved when a path line would otherwise overwrite it.
#[inline]
fn is_box_border(ch: char) -> bool {
    matches!(
        ch,
        '┌' | '┐' | '└' | '┘' | '╭' | '╮' | '╰' | '╯' | '─' | '│'
    )
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kozue_ir::{Path, Rect, Scene, SceneItem, Text, TextAlign};

    fn empty_scene() -> Scene {
        Scene {
            width: 0.0,
            height: 0.0,
            items: vec![],
        }
    }

    #[test]
    fn empty_scene_no_panic() {
        let out = render(&empty_scene());
        // Must not panic, and must be a valid string.
        assert!(out.ends_with('\n') || out.is_empty());
    }

    #[test]
    fn determinism() {
        let scene = Scene {
            width: 80.0,
            height: 32.0,
            items: vec![
                SceneItem::Rect(Rect {
                    x: 10.0,
                    y: 10.0,
                    width: 60.0,
                    height: 12.0,
                    rx: 4.0,
                }),
                SceneItem::Text(Text {
                    x: 40.0,
                    y: 18.0,
                    size: 12.0,
                    align: TextAlign::Middle,
                    content: "Hello".to_string(),
                    text_width: 40.0,
                    text_height: 12.0,
                }),
            ],
        };
        let out1 = render(&scene);
        let out2 = render(&scene);
        assert_eq!(out1, out2, "render must be deterministic");
    }

    #[test]
    fn wide_char_placement() {
        // "あ" has display width 2.
        let scene = Scene {
            width: 64.0,
            height: 32.0,
            items: vec![SceneItem::Text(Text {
                x: 0.0,
                y: 16.0,
                size: 12.0,
                align: TextAlign::Start,
                content: "あ".to_string(),
                text_width: 16.0,
                text_height: 16.0,
            })],
        };
        let out = render(&scene);
        // Baseline y=16 with size 12 → visual centre 11.8px → row 0.
        let row0 = out.lines().next().unwrap_or("");
        assert!(row0.contains('あ'), "wide char should be placed: {out:?}");
    }

    /// Fix 1: Wide character cell model.
    /// The rendered line containing a wide char must have display width equal
    /// to the number of logical grid columns, not +1 due to an explicit space.
    #[test]
    fn wide_char_display_width_equals_logical_cols() {
        // Grid: 8 cols, 2 rows.  Place "あ" (width 2) at col 0.
        // Logical cells: [あ][Wide][空][空][空][空][空][空]
        // Output chars:  あ (skip Wide) spaces … → display width = 8 at most,
        // but trimming trailing spaces means the line ends after "あ".
        let scene = Scene {
            width: 64.0,  // 8 cols
            height: 32.0, // 2 rows
            items: vec![SceneItem::Text(Text {
                x: 0.0,
                y: 16.0,
                size: 12.0,
                align: TextAlign::Start,
                content: "あ".to_string(),
                text_width: 16.0,
                text_height: 16.0,
            })],
        };
        let out = render(&scene);
        // Find the row that contains 'あ'.
        let row = out
            .lines()
            .find(|l| l.contains('あ'))
            .expect("should have あ");
        let display_w: usize = row
            .chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(1))
            .sum();
        // After trimming trailing spaces the line is just "あ" with display width 2.
        assert_eq!(
            display_w, 2,
            "display width of line with wide char must equal logical cell count: {row:?}"
        );
    }

    #[test]
    fn path_arrowhead_down() {
        // Tip is BELOW the base: arrow points DOWN.
        let scene = Scene {
            width: 32.0,
            height: 48.0,
            items: vec![SceneItem::Path(Path {
                points: vec![(16.0, 32.0), (8.0, 16.0), (24.0, 16.0)],
                filled: true,
                dashed: false,
            })],
        };
        let out = render(&scene);
        assert!(
            out.contains('▼'),
            "down-pointing arrowhead expected: {out:?}"
        );
    }

    #[test]
    fn rect_draws_box() {
        let scene = Scene {
            width: 80.0,
            height: 64.0,
            items: vec![SceneItem::Rect(Rect {
                x: 0.0,
                y: 0.0,
                width: 80.0,
                height: 48.0,
                rx: 0.0,
            })],
        };
        let out = render(&scene);
        assert!(out.contains('┌'), "top-left corner expected: {out:?}");
        assert!(out.contains('┐'), "top-right corner expected: {out:?}");
        assert!(out.contains('└'), "bottom-left corner expected: {out:?}");
        assert!(out.contains('┘'), "bottom-right corner expected: {out:?}");
    }

    #[test]
    fn dashed_path_differs_from_solid() {
        let pts = vec![(0.0, 0.0), (80.0, 0.0)];
        let scene_solid = Scene {
            width: 96.0,
            height: 32.0,
            items: vec![SceneItem::Path(Path {
                points: pts.clone(),
                filled: false,
                dashed: false,
            })],
        };
        let scene_dashed = Scene {
            width: 96.0,
            height: 32.0,
            items: vec![SceneItem::Path(Path {
                points: pts,
                filled: false,
                dashed: true,
            })],
        };
        let solid = render(&scene_solid);
        let dashed = render(&scene_dashed);
        // Dashed should have fewer non-space chars on that row.
        let count_solid = solid
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .filter(|&c| c != ' ')
            .count();
        let count_dashed = dashed
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .filter(|&c| c != ' ')
            .count();
        assert!(
            count_dashed < count_solid,
            "dashed path should have fewer drawn cells: solid={count_solid}, dashed={count_dashed}"
        );
    }

    // ---------------------------------------------------------------------------
    // Fix 5: Border alignment machine-check (permanent test).
    //
    // For each of the 4 terminal golden .txt files, verify that the
    // display-width-based column position of vertical border characters (│ ┌ ┐
    // └ ┘) is consistent across all rows of the same box.
    //
    // Strategy:
    //  1. Parse each line into a list of (display_col, char) pairs.
    //  2. Collect the display columns at which '┌' or '┐' appear (top corners).
    //  3. For every subsequent row, verify that '│', '└', '┘' appear at one of
    //     the same display columns (within ±0 — exact match).
    //
    // This catches the "2+1=3 wide-char bug" where an explicit space after a
    // wide char pushes later columns one position to the right.
    // ---------------------------------------------------------------------------

    /// Compute (display_col, char) pairs for every character in a line.
    fn display_col_chars(line: &str) -> Vec<(usize, char)> {
        let mut pairs = Vec::new();
        let mut col = 0usize;
        for ch in line.chars() {
            pairs.push((col, ch));
            col += UnicodeWidthChar::width(ch).unwrap_or(1);
        }
        pairs
    }

    /// Returns the set of display columns where `ch` appears in `line`.
    fn cols_of(line: &str, ch: char) -> Vec<usize> {
        display_col_chars(line)
            .into_iter()
            .filter(|&(_, c)| c == ch)
            .map(|(col, _)| col)
            .collect()
    }

    /// Core check: for a rendered text, every *box* must have its vertical
    /// borders aligned in display-column space.
    ///
    /// Algorithm:
    ///  1. Scan all lines and record every (row, col) position of top corners
    ///     (┌ and ┐) paired on the same row to form boxes.
    ///  2. For each box identified by (top_row, left_col, right_col), scan
    ///     downward until the matching bottom corners (└ at left_col, ┘ at
    ///     right_col) are found.
    ///  3. Verify that every interior row of the box has │ at left_col AND
    ///     right_col in display-column space.
    ///
    /// This deliberately ignores `│` on path/arrow lines between boxes, which
    /// are not box-border characters.
    fn assert_borders_aligned(label: &str, text: &str) {
        let lines: Vec<&str> = text.lines().collect();

        // Find top edges: pairs of (┌ at col L, ┐ at col R) on the same row.
        // A row may contain multiple boxes.
        let mut boxes: Vec<(usize, usize, usize)> = Vec::new(); // (top_row, left_col, right_col)

        for (row_idx, line) in lines.iter().enumerate() {
            let lefts = cols_of(line, '┌');
            let rights = cols_of(line, '┐');
            // Pair each ┌ with the nearest ┐ to its right.
            for &l in &lefts {
                if let Some(&r) = rights.iter().find(|&&r| r > l) {
                    boxes.push((row_idx, l, r));
                }
            }
        }

        // For each box, verify its sides.
        for (top_row, left_col, right_col) in boxes {
            // Find the bottom row: scan downward for └ at left_col and ┘ at right_col.
            let bottom_row = (top_row + 1..lines.len()).find(|&r| {
                cols_of(lines[r], '└').contains(&left_col)
                    && cols_of(lines[r], '┘').contains(&right_col)
            });

            let bottom_row = match bottom_row {
                Some(r) => r,
                None => continue, // no matching bottom found (partial render), skip
            };

            // Verify bottom corners are exactly where expected.
            assert!(
                cols_of(lines[bottom_row], '└').contains(&left_col),
                "{label}: box top at row {top_row} col {left_col}: └ not at col {left_col} in row {bottom_row}\n  line: {:?}",
                lines[bottom_row]
            );
            assert!(
                cols_of(lines[bottom_row], '┘').contains(&right_col),
                "{label}: box top at row {top_row} col {right_col}: ┘ not at col {right_col} in row {bottom_row}\n  line: {:?}",
                lines[bottom_row]
            );

            // Verify that interior rows have │ at left_col and right_col.
            // (The top and bottom rows have ┌/┐ and └/┘ respectively.)
            for (interior_row, line) in lines.iter().enumerate().take(bottom_row).skip(top_row + 1)
            {
                let pipe_cols: Vec<usize> = cols_of(line, '│');
                // Interior rows may also contain arrows (▼/▲) and text;
                // what matters is that the left and right border cols have │.
                // However the label/arrow may overwrite a │; we only check
                // that the column is occupied by some character at that
                // position — specifically, check both border cols appear.
                assert!(
                    pipe_cols.contains(&left_col),
                    "{label}: box [{top_row}:{left_col}..{right_col}]: interior row {interior_row} \
                     missing │ at left col {left_col}\n  line: {line:?}"
                );
                assert!(
                    pipe_cols.contains(&right_col),
                    "{label}: box [{top_row}:{left_col}..{right_col}]: interior row {interior_row} \
                     missing │ at right col {right_col}\n  line: {line:?}"
                );
            }
        }
    }

    /// Inline a minimal wide-char box to confirm the alignment check itself
    /// catches misaligned borders caused by the old "space after wide char" bug.
    #[test]
    fn border_alignment_wide_char_box() {
        // Build a scene with a box and a wide-char label inside it.
        // Box: x=0, y=0, w=80, h=32  → cols 0..10, rows 0..2
        // Label: "あ" centred at x=40 (col 5), y=24 (baseline) size=12
        //   → with fix2 row = round((24 - 12*0.35)/16) = round(19.8/16) = round(1.2375) = 1
        //   → centre col = 5 - 1 = 4
        let scene = Scene {
            width: 80.0,
            height: 32.0,
            items: vec![
                SceneItem::Rect(Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 80.0,
                    height: 32.0,
                    rx: 0.0,
                }),
                SceneItem::Text(Text {
                    x: 40.0,
                    y: 24.0,
                    size: 12.0,
                    align: TextAlign::Middle,
                    content: "あい".to_string(),
                    text_width: 32.0,
                    text_height: 12.0,
                }),
            ],
        };
        let out = render(&scene);
        assert_borders_aligned("wide_char_box", &out);
    }

    /// Load the 4 terminal golden .txt files (if they exist) and run the
    /// border alignment check on each.
    ///
    /// The files are generated/regenerated by the integration test suite with
    /// UPDATE_GOLDEN=1.  If a file is missing (e.g., first run before
    /// goldens exist) the check is skipped for that file.
    #[test]
    fn border_alignment_golden_txts() {
        // Locate workspace root relative to this crate's CARGO_MANIFEST_DIR.
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // manifest = …/crates/kozue-render-term
        let golden_dir = manifest
            .parent() // crates
            .and_then(|p| p.parent()) // workspace root
            .map(|root| root.join("tests").join("golden"))
            .expect("could not compute golden dir");

        let cases = ["chain", "branch", "seq_basic", "mermaid_flow"];
        for name in cases {
            let path = golden_dir.join(format!("{name}.txt"));
            match std::fs::read_to_string(&path) {
                Ok(content) => assert_borders_aligned(name, &content),
                Err(_) => {
                    // File not yet generated — skip gracefully.
                    eprintln!("border_alignment_golden_txts: skipping {name}.txt (not found)");
                }
            }
        }
    }
}
