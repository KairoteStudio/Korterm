// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Terminal buffer — a grid of cells with scrollback.
//!
//! Mirrors xterm.js `Buffer` + `BufferLine` + `CellData`:
//! - [`Cell`] holds a UTF-32 codepoint (or combined string), display width
//!   (0/1/2 for combining/narrow/wide), and packed [`Attr`].
//! - [`BufferLine`] is a row of cells, grown to the terminal width.
//! - [`Buffer`] owns the line ring-buffer, cursor position, scroll state
//!   and the "alternate screen" concept.

use crate::attr::Attr;

/// A single screen cell.
#[derive(Clone, Debug)]
pub struct Cell {
    /// UTF-32 codepoint, or 0 for an empty cell.
    pub codepoint: u32,
    /// Display width: 0 = combining, 1 = narrow, 2 = wide (CJK).
    pub width: u8,
    /// Combined character string (for grapheme clusters), empty if single.
    pub combined: String,
    /// Packed text attributes (colour + style).
    pub attr: Attr,
}

impl Cell {
    pub const BLANK: Cell = Cell {
        codepoint: 0,
        width: 1,
        combined: String::new(),
        attr: Attr::DEFAULT,
    };

    pub fn is_empty(&self) -> bool {
        self.codepoint == 0 && self.combined.is_empty()
    }

    /// The visible character(s) for this cell.
    pub fn chars(&self) -> String {
        if !self.combined.is_empty() {
            self.combined.clone()
        } else if self.codepoint != 0 {
            char::from_u32(self.codepoint)
                .map(|c| c.to_string())
                .unwrap_or_default()
        } else {
            ' '.to_string()
        }
    }
}

/// One row of the terminal.
#[derive(Clone, Debug)]
pub struct BufferLine {
    pub cells: Vec<Cell>,
    pub is_wrapped: bool,
}

impl BufferLine {
    pub fn new(width: usize) -> Self {
        BufferLine {
            cells: vec![Cell::BLANK; width],
            is_wrapped: false,
        }
    }

    pub fn resize(&mut self, width: usize) {
        if width > self.cells.len() {
            self.cells.resize(width, Cell::BLANK);
        } else {
            self.cells.truncate(width);
        }
    }

    pub fn clear(&mut self, attr: Attr) {
        for c in &mut self.cells {
            *c = Cell::BLANK;
            c.attr = attr;
        }
        self.is_wrapped = false;
    }
}

/// The terminal buffer: a ring of lines with cursor + scroll state.
#[derive(Clone, Debug)]
pub struct Buffer {
    /// All lines including scrollback (index 0 = oldest).
    pub lines: Vec<BufferLine>,
    /// Number of visible rows (terminal height).
    pub rows: usize,
    /// Number of visible columns (terminal width).
    pub cols: usize,
    /// Maximum scrollback lines (0 = no scrollback).
    pub scrollback: usize,

    // cursor
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub saved_cursor: (usize, usize),

    // scroll region
    pub scroll_top: usize,
    pub scroll_bottom: usize,

    /// Current text attributes applied to newly written cells.
    pub cur_attr: Attr,

    /// Whether the cursor is visible (DECSET ?25 / DECRST ?25).
    pub cursor_visible: bool,

    /// Lines discarded from the top by scrollback-cap enforcement since
    /// the last consumption. `Terminal::pump` subtracts this from
    /// absolute line indices (search matches, `view_offset`) so they
    /// keep pointing at the same text after history is trimmed.
    pub dropped: usize,

    /// True while the program owns the alternate screen (`ESC[?1049h`,
    /// `?47h`, `?1047h`) — vim, top, htop, less and friends. Used to
    /// tell "a program is running" apart from "the shell is at a
    /// prompt", so closing a tab can warn first.
    pub alt_screen: bool,

    /// Shell-integration state (OSC 133). `true` right after the prompt
    /// was drawn, `false` from the moment a command starts until the
    /// next prompt. `starts_prompted` is the initial value.
    pub at_prompt: bool,
}

impl Buffer {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        let mut buf = Buffer {
            lines: Vec::with_capacity(rows + scrollback),
            rows,
            cols,
            scrollback,
            cursor_x: 0,
            cursor_y: 0,
            saved_cursor: (0, 0),
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            cur_attr: Attr::DEFAULT,
            cursor_visible: true,
            dropped: 0,
            alt_screen: false,
            at_prompt: true,
        };
        for _ in 0..rows {
            buf.lines.push(BufferLine::new(cols));
        }
        buf
    }

    /// Resize the column count — truncate/extend every line to `cols`
    /// and clamp the cursor's X.
    ///
    /// **Ordering**: `Terminal::flush_pty_resize` calls this BEFORE the
    /// PTY is resized — the buffer must adopt the new width first so the
    /// shell's post-SIGWINCH output is interpreted against a viewport
    /// that already matches the shell's; doing it the other way round
    /// makes the shell's still-wide output wrap at the new (narrower)
    /// width, producing "duplicate" lines.
    ///
    /// **Performance**: Only resizes the visible viewport, not the entire
    /// scrollback. This avoids O(n) operations when there are thousands
    /// of scrollback lines. Scrollback lines are resized lazily when
    /// they become visible.
    pub fn resize_cols(&mut self, cols: usize) {
        if cols == self.cols {
            return; // No change needed
        }
        // Only resize the visible viewport. Scrollback lines will be
        // resized lazily when they become visible.
        let start = self.lines.len().saturating_sub(self.rows);
        for line in &mut self.lines[start..] {
            if cols > line.cells.len() {
                line.cells.resize(cols, Cell::BLANK);
            } else {
                line.cells.truncate(cols);
            }
        }
        self.cols = cols;
        if self.cursor_x >= cols {
            self.cursor_x = cols.saturating_sub(1);
        }
    }

    /// Resize the row count — mirrors xterm.js `Buffer.resize` row logic:
    ///
    /// **Shrinking**: process one row at a time.  If there are blank
    /// lines below the cursor (`lines.len() > ybase + cursor_y + 1`),
    /// pop them from the bottom; otherwise the cursor's own line is
    /// at the bottom — leave it in place so it becomes scrollback
    /// (our scrollback is implicit: `lines.len() - rows`).
    ///
    /// **Growing**: if there is scrollback above (`ybase > 0`) and no
    /// blank lines below the cursor, pull a line back from scrollback
    /// (just let `rows` grow — the implicit scrollback shrinks
    /// automatically); otherwise push a blank line at the bottom.
    ///
    /// In both cases the cursor's absolute line (`ybase + cursor_y`)
    /// is preserved; only `cursor_y` is clamped to the new viewport.
    pub fn resize_rows(&mut self, rows: usize) {
        if rows == self.rows {
            return;
        }

        if rows > self.rows {
            // ---- Growing ----
            let mut add_to_y: usize = 0;
            for _ in self.rows..rows {
                let ybase = self.lines.len().saturating_sub(self.rows);
                if self.lines.len() < rows + ybase {
                    if ybase > 0
                        && self.lines.len() <= ybase + self.cursor_y + add_to_y + 1
                    {
                        // Scrollback exists and no blank lines below
                        // cursor → restore one line from scrollback.
                        // Our scrollback is implicit, so just let
                        // `rows` grow at the end; the visible window
                        // expands upward automatically.
                        add_to_y += 1;
                    } else {
                        // No scrollback or blank lines exist below
                        // cursor → push a blank line at the bottom.
                        self.lines.push(BufferLine::new(self.cols));
                    }
                }
            }
            self.rows = rows;
            self.scroll_bottom = rows.saturating_sub(1);
            if self.scroll_top >= rows {
                self.scroll_top = 0;
            }
            self.cursor_y = (self.cursor_y + add_to_y).min(rows.saturating_sub(1));
        } else {
            // ---- Shrinking ----
            for _ in rows..self.rows {
                let ybase = self.lines.len().saturating_sub(self.rows);
                if self.lines.len() > rows + ybase {
                    if self.lines.len() > ybase + self.cursor_y + 1 {
                        // Blank line below cursor → remove it.
                        self.lines.pop();
                    } else {
                        // Cursor is at the bottom → its line becomes
                        // scrollback.  Our scrollback is implicit, so
                        // just let `rows` shrink at the end.
                    }
                }
            }
            self.rows = rows;
            self.scroll_bottom = rows.saturating_sub(1);
            if self.scroll_top >= rows {
                self.scroll_top = 0;
            }
            self.cursor_y = self.cursor_y.min(rows.saturating_sub(1));
        }

        // Enforce the scrollback cap so memory doesn't grow unbounded.
        let max_lines = rows + self.scrollback;
        if self.lines.len() > max_lines {
            let drop = self.lines.len() - max_lines;
            self.lines.drain(0..drop);
            self.dropped += drop;
        }
    }

    /// Full resize — both cols and rows at once.  Used by call-sites
    /// that resize the PTY immediately (no debounce), e.g.
    /// `reflow_active_terminal` for discrete events like status-bar
    /// toggle.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.resize_cols(cols);
        self.resize_rows(rows);
    }

    /// Index into `lines` for a given visible row (0 = top of screen).
    /// With scrollback, visible rows start at `lines.len() - rows`.
    pub fn visible_line(&self, row: usize) -> &BufferLine {
        let start = self.lines.len().saturating_sub(self.rows);
        &self.lines[start + row]
    }

    pub fn visible_line_mut(&mut self, row: usize) -> &mut BufferLine {
        let start = self.lines.len().saturating_sub(self.rows);
        &mut self.lines[start + row]
    }

    /// Current cursor line (mutable).
    pub fn cur_line_mut(&mut self) -> &mut BufferLine {
        let start = self.lines.len().saturating_sub(self.rows);
        &mut self.lines[start + self.cursor_y]
    }

    /// Scroll up by `n` lines within the scroll region, filling new lines
    /// at the bottom with blanks.
    pub fn scroll_up(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        let start = self.lines.len().saturating_sub(self.rows);
        let top = start + self.scroll_top;
        let bot = start + self.scroll_bottom;
        let n = n.min(self.scroll_bottom - self.scroll_top + 1);

        // Shift lines up within [top, bot].
        for i in 0..=(bot - top - n) {
            self.lines[top + i] = self.lines[top + i + n].clone();
        }
        // Fill the freed lines at the bottom with blanks.
        for i in (bot - n + 1)..=bot {
            self.lines[i] = BufferLine::new(self.cols);
            self.lines[i].clear(self.cur_attr);
        }
    }

    /// Scroll down by `n` lines (content moves down, top gets blanks).
    pub fn scroll_down(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        let start = self.lines.len().saturating_sub(self.rows);
        let top = start + self.scroll_top;
        let bot = start + self.scroll_bottom;
        let n = n.min(self.scroll_bottom - self.scroll_top + 1);

        // Shift lines down within [top, bot].
        for i in (top + n..=bot).rev() {
            self.lines[i] = self.lines[i - n].clone();
        }
        // Fill the freed lines at the top with blanks.
        for i in top..top + n {
            self.lines[i] = BufferLine::new(self.cols);
            self.lines[i].clear(self.cur_attr);
        }
    }

    /// Move cursor to the start of the next line, scrolling if needed.
    ///
    /// When the cursor is at the bottom of a **full-viewport** scroll
    /// region, a fresh blank line is pushed onto `lines` so the old top
    /// line flows into scrollback — terminal history is preserved.
    /// For a **partial** scroll region (DECSTBM), `scroll_up` is used
    /// and the top line of the region is discarded as before.
    pub fn line_feed(&mut self) {
        if self.cursor_y == self.scroll_bottom {
            if self.scroll_top == 0 && self.scroll_bottom == self.rows - 1 {
                self.lines.push(BufferLine::new(self.cols));
                let max_lines = self.rows + self.scrollback;
                if self.lines.len() > max_lines {
                    let drop = self.lines.len() - max_lines;
                    self.lines.drain(0..drop);
                    self.dropped += drop;
                }
            } else {
                self.scroll_up(1);
            }
        } else if self.cursor_y < self.rows - 1 {
            self.cursor_y += 1;
        }
        self.cursor_x = 0;
    }

    /// Carriage return — move cursor to column 0.
    pub fn carriage_return(&mut self) {
        self.cursor_x = 0;
    }

    /// Write a character at the cursor, advancing the cursor.
    /// Handles wide chars (width 2) and line wrap.
    pub fn write_char(&mut self, codepoint: u32, width: u8, combined: &str) {
        let w = width as usize;
        // Wrap if the char doesn't fit on the current line.
        if self.cursor_x + w > self.cols {
            self.cur_line_mut().is_wrapped = true;
            self.line_feed();
        }

        let cx = self.cursor_x;
        let attr = self.cur_attr;
        let line = self.cur_line_mut();
        // For wide chars, clear the following cell too.
        if cx < line.cells.len() {
            line.cells[cx] = Cell {
                codepoint,
                width,
                combined: combined.to_string(),
                attr,
            };
            if w == 2 && cx + 1 < line.cells.len() {
                line.cells[cx + 1] = Cell {
                    codepoint: 0,
                    width: 0, // continuation cell
                    combined: String::new(),
                    attr,
                };
            }
        }

        self.cursor_x += w;
        // When the cursor reaches the last column, leave it at `cols`
        // (the "pending wrap" state) instead of clamping to `cols - 1`.
        // The next `write_char` will then see `cursor_x + w > cols` and
        // wrap to the next line.  This matches the behaviour of real
        // terminals (xterm/vt100) and avoids overwriting the last cell.
        if self.cursor_x >= self.cols {
            self.cursor_x = self.cols;
        }
    }

    /// Backspace: move cursor left one column (no wrap).
    pub fn backspace(&mut self) {
        if self.cursor_x > 0 {
            self.cursor_x -= 1;
        }
    }

    /// Move cursor to absolute position (1-based in ANSI, converted here).
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_y = row.saturating_sub(1).min(self.rows - 1);
        self.cursor_x = col.saturating_sub(1).min(self.cols - 1);
    }

    /// Save / restore cursor (DECSC/DECRC).
    pub fn save_cursor(&mut self) {
        self.saved_cursor = (self.cursor_x, self.cursor_y);
    }
    pub fn restore_cursor(&mut self) {
        let (x, y) = self.saved_cursor;
        self.cursor_x = x.min(self.cols - 1);
        self.cursor_y = y.min(self.rows - 1);
    }

    /// Erase from cursor to end of line.
    pub fn erase_to_end_of_line(&mut self) {
        let cx = self.cursor_x;
        let attr = self.cur_attr;
        let line = self.cur_line_mut();
        for i in cx..line.cells.len() {
            line.cells[i] = Cell::BLANK;
            line.cells[i].attr = attr;
        }
    }

    /// Erase from start of line to cursor (inclusive).
    pub fn erase_to_cursor_in_line(&mut self) {
        let cx = self.cursor_x;
        let attr = self.cur_attr;
        let line = self.cur_line_mut();
        for i in 0..=cx.min(line.cells.len().saturating_sub(1)) {
            line.cells[i] = Cell::BLANK;
            line.cells[i].attr = attr;
        }
    }

    /// Erase entire line at cursor.
    pub fn erase_line(&mut self) {
        let attr = self.cur_attr;
        self.cur_line_mut().clear(attr);
    }

    /// Erase from cursor to end of screen.
    pub fn erase_to_end_of_screen(&mut self) {
        self.erase_to_end_of_line();
        let cy = self.cursor_y;
        let attr = self.cur_attr;
        let start = self.lines.len().saturating_sub(self.rows);
        for r in cy + 1..self.rows {
            self.lines[start + r].clear(attr);
        }
    }

    /// Erase from start of screen to cursor.
    pub fn erase_to_cursor_in_screen(&mut self) {
        self.erase_to_cursor_in_line();
        let cy = self.cursor_y;
        let attr = self.cur_attr;
        let start = self.lines.len().saturating_sub(self.rows);
        for r in 0..cy {
            self.lines[start + r].clear(attr);
        }
    }

    /// Erase entire screen.
    pub fn erase_screen(&mut self) {
        let attr = self.cur_attr;
        let start = self.lines.len().saturating_sub(self.rows);
        for r in 0..self.rows {
            self.lines[start + r].clear(attr);
        }
    }

    /// Insert `n` blank lines at the cursor row (scrolling lines below down).
    pub fn insert_lines(&mut self, n: usize) {
        let start = self.lines.len().saturating_sub(self.rows);
        let cy = start + self.cursor_y;
        let bot = start + self.scroll_bottom;
        let n = n.min(bot - cy + 1);
        // Shift lines down.
        for i in (cy + n..=bot).rev() {
            self.lines[i] = self.lines[i - n].clone();
        }
        // Insert blanks.
        let attr = self.cur_attr;
        for i in cy..cy + n {
            self.lines[i] = BufferLine::new(self.cols);
            self.lines[i].clear(attr);
        }
    }

    /// Delete `n` lines at the cursor row (scrolling lines below up).
    pub fn delete_lines(&mut self, n: usize) {
        let start = self.lines.len().saturating_sub(self.rows);
        let cy = start + self.cursor_y;
        let bot = start + self.scroll_bottom;
        let n = n.min(bot - cy + 1);
        // Shift lines up.
        for i in 0..=(bot - cy - n) {
            self.lines[cy + i] = self.lines[cy + i + n].clone();
        }
        // Fill bottom with blanks.
        let attr = self.cur_attr;
        for i in (bot - n + 1)..=bot {
            self.lines[i] = BufferLine::new(self.cols);
            self.lines[i].clear(attr);
        }
    }

    /// Insert `n` blank characters at cursor (shifting right).
    pub fn insert_chars(&mut self, n: usize) {
        let cx = self.cursor_x;
        let cols = self.cols;
        let attr = self.cur_attr;
        let n = n.min(cols - cx);
        let line = self.cur_line_mut();
        // Shift right.
        for i in (cx + n..cols).rev() {
            line.cells[i] = line.cells[i - n].clone();
        }
        // Insert blanks.
        for i in cx..cx + n {
            line.cells[i] = Cell::BLANK;
            line.cells[i].attr = attr;
        }
    }

    /// Delete `n` characters at cursor (shifting left).
    pub fn delete_chars(&mut self, n: usize) {
        let cx = self.cursor_x;
        let cols = self.cols;
        let attr = self.cur_attr;
        let n = n.min(cols - cx);
        let line = self.cur_line_mut();
        // Shift left.
        for i in cx..cols - n {
            line.cells[i] = line.cells[i + n].clone();
        }
        // Fill right with blanks.
        for i in cols - n..cols {
            line.cells[i] = Cell::BLANK;
            line.cells[i].attr = attr;
        }
    }

    /// Erase `n` characters from cursor (replacing with blanks, no shift).
    pub fn erase_chars(&mut self, n: usize) {
        let cx = self.cursor_x;
        let n = n.min(self.cols - cx);
        let attr = self.cur_attr;
        let line = self.cur_line_mut();
        for i in cx..cx + n {
            line.cells[i] = Cell::BLANK;
            line.cells[i].attr = attr;
        }
    }
}