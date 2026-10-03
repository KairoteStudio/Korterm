// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! A terminal emulator crate for Iced — a Rust reimplementation of the
//! core ideas from xterm.js.
//!
//! Architecture (mirrors xterm.js):
//! - [`buffer`] — cell grid, cursor, scrollback, erase/insert/scroll ops
//! - [`parser`] — VT100/xterm escape-sequence state machine
//! - [`handler`] — applies parsed actions to the buffer
//! - [`pty`] — spawns a child shell on a pseudo-terminal
//! - [`Terminal`] — ties it all together, exposes a simple `feed`/`input` API

pub mod attr;
pub mod buffer;
pub mod handler;
pub mod parser;
pub mod pty;
pub mod widget;

use buffer::Buffer;
use parser::Parser;
use pty::PtySession;

/// A terminal emulator instance backed by a live PTY.
pub struct Terminal {
    pub buf: Buffer,
    parser: Parser,
    pub pty: Option<PtySession>,
    /// Window title set via OSC 0 / 2.
    pub title: String,
    /// Previous cursor position (x, y) — the "from" point for the
    /// smooth-move animation.
    pub prev_cursor: (usize, usize),
    /// Animation progress 0..1 (1 = settled at the current cursor pos).
    pub cursor_anim_t: f32,
    /// Whether the cursor smooth-move animation is currently active.
    cursor_anim_active: bool,
    /// Blink phase: true = cursor visible (on-phase), false = off-phase.
    cursor_blink_visible: bool,
    /// Last blink toggle instant.
    last_blink: std::time::Instant,
    /// Active text selection, in viewport-relative cell coords
    /// `(cell_x, viewport_row)` where row 0 = top of the visible
    /// area. Viewport-relative (not absolute line index) so the
    /// selection stays valid when the scrollback ring drops old
    /// lines from the front (`Buffer` drains `0..drop`).
    pub selection: Option<Selection>,
    /// Pending PTY resize, deferred so that rapid panel-resize
    /// events (e.g. dragging the splitter) don't flood the shell
    /// with SIGWINCH signals — each of which would trigger a full
    /// prompt redraw and push spurious "duplicate" lines into the
    /// buffer.  The host calls [`flush_pty_resize`] after a short
    /// debounce to apply the latest size in one shot.
    pending_pty_resize: Option<(u16, u16)>,
    /// Scrollback viewport offset: how many lines the view is scrolled up
    /// from the live bottom (0 = following the bottom, like a normal
    /// terminal).
    pub view_offset: usize,
    /// Active search (query + all matches + current match index).
    pub search: Option<Search>,
}

/// A single search hit: absolute line index + visual column range.
#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
    /// The matched text (original case), for `file:line` classification.
    pub text: String,
}

/// Active search state.
#[derive(Clone, Debug)]
pub struct Search {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    /// Index of the currently focused match.
    pub current: usize,
}

impl Clone for Terminal {
    fn clone(&self) -> Self {
        Terminal {
            buf: self.buf.clone(),
            parser: self.parser.clone(),
            pty: None, // Skip PTY — it can't be cloned
            title: self.title.clone(),
            prev_cursor: self.prev_cursor,
            cursor_anim_t: self.cursor_anim_t,
            cursor_anim_active: self.cursor_anim_active,
            cursor_blink_visible: self.cursor_blink_visible,
            last_blink: self.last_blink,
            selection: self.selection,
            pending_pty_resize: self.pending_pty_resize,
            view_offset: self.view_offset,
            search: self.search.clone(),
        }
    }
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal")
            .field("buf", &self.buf)
            .field("title", &self.title)
            .field("prev_cursor", &self.prev_cursor)
            .field("cursor_anim_t", &self.cursor_anim_t)
            .field("selection", &self.selection)
            .finish_non_exhaustive()
    }
}

/// A text selection span: `(cell_x, viewport_row)` start and end.
/// Normalized on read so `start <= end` (row-major).
#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

impl Terminal {
    /// Create a new terminal at the given grid size with the default shell.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self::with_shell(cols, rows, None, None)
    }

    /// A PTY-less terminal — a plain empty buffer. Used for pseudo-sessions
    /// like the settings tab, which render custom UI instead of terminal
    /// content but still participate in tab drag/select/close logic.
    pub fn headless(cols: usize, rows: usize) -> Self {
        Terminal {
            buf: Buffer::new(cols, rows, 5000),
            parser: Parser::new(),
            pty: None,
            title: String::new(),
            prev_cursor: (0, 0),
            cursor_anim_t: 1.0,
            cursor_anim_active: false,
            cursor_blink_visible: true,
            last_blink: std::time::Instant::now(),
            selection: None,
            pending_pty_resize: None,
            view_offset: 0,
            search: None,
        }
    }

    /// Create a new terminal at the given grid size with a specific shell.
    /// When `program` is `None`, uses the user's default shell.
    /// When `cwd` is `None`, the shell starts in the user's home directory.
    pub fn with_shell(
        cols: usize,
        rows: usize,
        program: Option<&str>,
        cwd: Option<&str>,
    ) -> Self {
        let mut term = Terminal {
            buf: Buffer::new(cols, rows, 5000),
            parser: Parser::new(),
            pty: None,
            title: "Terminal".to_string(),
            prev_cursor: (0, 0),
            cursor_anim_t: 1.0,
            cursor_anim_active: false,
            cursor_blink_visible: true,
            last_blink: std::time::Instant::now(),
            selection: None,
            pending_pty_resize: None,
            view_offset: 0,
            search: None,
        };
        match PtySession::spawn_with(cols as u16, rows as u16, program, cwd) {
            Ok(session) => term.pty = Some(session),
            Err(e) => {
                // PTY spawn failed — write an error banner so the user
                // sees something instead of a blank screen.
                let msg = format!(
                    "\r\n\x1b[31mFailed to spawn shell: {}\x1b[0m\r\n",
                    e
                );
                let actions = term.parser.feed(msg.as_bytes());
                handler::InputHandler::apply(&mut term.buf, &actions);
            }
        }
        term
    }

    /// Async version of [`with_shell`] — spawns the shell on a blocking
    /// thread so the UI thread is never blocked. Returns a Task that
    /// delivers a `Result<Terminal, String>` (Ok on success, Err with
    /// error message on failure).
    pub fn with_shell_async(
        cols: usize,
        rows: usize,
        program: Option<&str>,
        cwd: Option<&str>,
    ) -> impl std::future::Future<Output = Self> {
        let program = program.map(|s| s.to_string());
        let cwd = cwd.map(|s| s.to_string());
        async move {
            let mut term = Terminal {
                buf: Buffer::new(cols, rows, 5000),
                parser: Parser::new(),
                pty: None,
                title: "Terminal".to_string(),
                prev_cursor: (0, 0),
                cursor_anim_t: 1.0,
                cursor_anim_active: false,
                cursor_blink_visible: true,
                last_blink: std::time::Instant::now(),
                selection: None,
                pending_pty_resize: None,
                view_offset: 0,
                search: None,
            };
            match PtySession::spawn_with_async(
                cols as u16,
                rows as u16,
                program.as_deref(),
                cwd.as_deref(),
            )
            .await
            {
                Ok(session) => term.pty = Some(session),
                Err(e) => {
                    let msg = format!(
                        "\r\n\x1b[31mFailed to spawn shell: {}\x1b[0m\r\n",
                        e
                    );
                    let actions = term.parser.feed(msg.as_bytes());
                    handler::InputHandler::apply(&mut term.buf, &actions);
                }
            }
            term
        }
    }

    /// Pump pending PTY output through the parser into the buffer.
    /// Call this on every UI tick (e.g. iced `subscription`).
    /// Returns `true` if any new bytes were processed (so the host can
    /// decide whether to auto-scroll to the bottom).
    pub fn pump(&mut self) -> bool {
        let Some(pty) = &self.pty else { return false };
        let data = pty.drain();
        if data.is_empty() {
            return false;
        }
        // Capture the cursor position before processing so we can detect
        // movement and start the smooth-move animation.
        let prev = (self.buf.cursor_x, self.buf.cursor_y);
        let actions = self.parser.feed(&data);
        handler::InputHandler::apply(&mut self.buf, &actions);
        // Scrollback lines were trimmed from the top while parsing this
        // batch — shift absolute line indices (viewport offset, search
        // matches) so they keep pointing at the same text.
        let dropped = std::mem::take(&mut self.buf.dropped);
        if dropped > 0 {
            self.view_offset = self.view_offset.saturating_sub(dropped);
            // Keep the selection glued to its text: shift it along with
            // the history that just scrolled out; drop it once fully gone.
            if let Some(sel) = &mut self.selection {
                if sel.start.1 < dropped && sel.end.1 < dropped {
                    self.selection = None;
                } else {
                    sel.start.1 = sel.start.1.saturating_sub(dropped);
                    sel.end.1 = sel.end.1.saturating_sub(dropped);
                }
            }
            if let Some(s) = &mut self.search {
                let mut removed_before_current = 0usize;
                let mut kept = Vec::with_capacity(s.matches.len());
                for (i, m) in s.matches.drain(..).enumerate() {
                    if m.line >= dropped {
                        let mut m = m;
                        m.line -= dropped;
                        kept.push(m);
                    } else if i < s.current {
                        removed_before_current += 1;
                    }
                }
                if kept.is_empty() {
                    // Every hit scrolled out of history — drop the search.
                    self.search = None;
                } else {
                    s.current = s
                        .current
                        .saturating_sub(removed_before_current)
                        .min(kept.len() - 1);
                    s.matches = kept;
                }
            }
        }
        let curr = (self.buf.cursor_x, self.buf.cursor_y);
        if curr != prev {
            self.prev_cursor = prev;
            self.cursor_anim_t = 0.0;
            self.cursor_anim_active = true;
            // JediTerm's `TerminalCursor.cursorChanged()`: any cursor
            // movement forces the cursor immediately into the visible
            // (on) phase and restarts the blink timer, so the cursor
            // never sits in the off-phase right after it moves.
            self.cursor_blink_visible = true;
            self.last_blink = std::time::Instant::now();
        }
        true
    }

    /// Send keyboard input to the shell.
    pub fn input(&mut self, data: &[u8]) {
        if let Some(pty) = &mut self.pty {
            let _ = pty.write(data);
        }
    }

    /// Resize the terminal grid.  **Neither cols nor rows are applied
    /// to the buffer immediately** — both are deferred to
    /// [`flush_pty_resize`] so the buffer stays in sync with the shell
    /// until the PTY is actually resized.
    ///
    /// Why defer *both* cols and rows?  During a rapid panel-resize
    /// (e.g. dragging the splitter), changing the buffer's rows
    /// before the shell knows about the new height causes the shell's
    /// output (still using the old rows) to be interpreted against the
    /// new viewport.  This shifts the cursor — and when the cursor's
    /// row is cropped into scrollback, `cursor_y` is clamped to 0
    /// while the real cursor line is in scrollback, so the shell's
    /// subsequent redraw (after SIGWINCH) writes to the wrong row and
    /// produces "duplicate" lines.  Deferring rows until the PTY is
    /// resized keeps the buffer and shell in lockstep.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.pending_pty_resize = Some((cols as u16, rows as u16));
    }

    /// Apply a pending PTY resize, if any.  The host should call this
    /// after a short debounce following the last [`resize`].
    ///
    /// The resize follows the same ordering as xterm.js's
    /// `CoreTerminal.resize`:
    /// 1. **Pump** all pending PTY output through the parser (with the
    ///    *old* dimensions) so the buffer is fully up-to-date.
    /// 2. **Resize the buffer** (cols + rows) to the new dimensions.
    /// 3. **Resize the PTY** last, which sends SIGWINCH to the shell.
    ///
    /// This order is critical: if the PTY were resized first, the shell
    /// would receive SIGWINCH and immediately start redrawing at the
    /// *new* width — but the buffer would still be at the *old* width,
    /// so the redraw output would be parsed with wrong dimensions and
    /// produce duplicate / empty lines.  By resizing the buffer first,
    /// the shell's post-SIGWINCH redraw is always parsed at the correct
    /// width.
    pub fn flush_pty_resize(&mut self) {
        if let Some((c, r)) = self.pending_pty_resize.take() {
            // 1. Flush any pending shell output at the old dimensions.
            self.pump();
            // 2. Resize the buffer to the new dimensions.
            self.buf.resize_cols(c as usize);
            self.buf.resize_rows(r as usize);
            // 3. Resize the PTY last — the shell gets SIGWINCH and
            //    redraws at the new width, which the next `pump` will
            //    parse correctly.
            if let Some(pty) = &self.pty {
                let _ = pty.resize(c, r);
            }
        }
    }

    /// Whether a deferred PTY resize is pending (i.e. [`resize`] was
    /// called but [`flush_pty_resize`] hasn't run yet).
    pub fn has_pending_pty_resize(&self) -> bool {
        self.pending_pty_resize.is_some()
    }

    /// Is the child shell still running?
    pub fn is_alive(&mut self) -> bool {
        self.pty.as_mut().map(|p| p.is_alive()).unwrap_or(false)
    }

    /// Advance the cursor smooth-move animation by `dt_ms` milliseconds.
    /// Returns `true` while the animation is still active.
    pub fn advance_cursor_anim(&mut self, dt_ms: f32) -> bool {
        if self.cursor_anim_active {
            self.cursor_anim_t += dt_ms / 120.0;
            if self.cursor_anim_t >= 1.0 {
                self.cursor_anim_t = 1.0;
                self.cursor_anim_active = false;
            }
        }
        self.cursor_anim_active
    }

    /// Whether the cursor animation is currently active.
    pub fn is_cursor_animating(&self) -> bool {
        self.cursor_anim_active
    }

    /// Returns the interpolated cursor position (x, y) in cell units,
    /// or `None` when the cursor is hidden (DECSET ?25 off).
    pub fn cursor_render_pos(&self) -> Option<(f32, f32)> {
        if !self.buf.cursor_visible {
            return None;
        }
        let t = ease_out_cubic(self.cursor_anim_t);
        let px = self.prev_cursor.0 as f32;
        let py = self.prev_cursor.1 as f32;
        let cx = self.buf.cursor_x as f32;
        let cy = self.buf.cursor_y as f32;
        Some((px + (cx - px) * t, py + (cy - py) * t))
    }

    /// Tick the cursor blink. When `focused`, the cursor toggles on/off
    /// every `BLINK_INTERVAL`; when not focused the blink phase is
    /// frozen and kept visible so that, on refocus, the cursor starts
    /// solid and the blink cycle restarts cleanly (mirrors JediTerm's
    /// `changeStateIfNeeded`, which early-returns when the panel has no
    /// focus).
    pub fn tick_blink(&mut self, focused: bool) {
        const BLINK_INTERVAL: std::time::Duration =
            std::time::Duration::from_millis(530);
        if focused {
            if self.last_blink.elapsed() >= BLINK_INTERVAL {
                self.cursor_blink_visible = !self.cursor_blink_visible;
                self.last_blink = std::time::Instant::now();
            }
        } else {
            self.cursor_blink_visible = true;
            self.last_blink = std::time::Instant::now();
        }
    }

    /// How the cursor should be rendered, mirroring JediTerm's
    /// `TerminalCursorState`:
    /// - [`CursorRenderState::Showing`] — focused and in the blink
    ///   on-phase (or the cursor just moved): draw a solid block.
    /// - [`CursorRenderState::NoFocus`] — the terminal is not focused:
    ///   draw a hollow rectangle outline so the user can still see
    ///   where the cursor is, without it blinking.
    /// - [`CursorRenderState::Hidden`] — DECSET ?25 turned the cursor
    ///   off, or focused and in the blink off-phase: draw nothing.
    pub fn cursor_render_state(&self, focused: bool) -> CursorRenderState {
        if !self.buf.cursor_visible {
            return CursorRenderState::Hidden;
        }
        if !focused {
            return CursorRenderState::NoFocus;
        }
        if self.cursor_blink_visible {
            CursorRenderState::Showing
        } else {
            CursorRenderState::Hidden
        }
    }

    /// Begin a new selection at the given viewport cell. The anchor is
    /// both the start and end — dragging later extends `end`.
    ///
    /// Rows are stored as **absolute buffer line indices** so the
    /// selection stays glued to the text when the user scrolls,
    /// instead of floating at a fixed screen position.
    ///
    /// Mapping: viewport row `vy` → absolute = (total - rows - view_offset) + vy.
    pub fn start_selection(&mut self, cx: usize, vy: usize) {
        let total = self.buf.lines.len();
        let offset = total.saturating_sub(self.buf.rows);
        let line = offset.saturating_sub(self.view_offset) + vy;
        self.selection = Some(Selection {
            start: (cx, line),
            end: (cx, line),
        });
    }

    /// Extend the active selection's end to the given viewport cell.
    /// No-op if no selection is active. `vy` is a viewport row and is
    /// converted to an absolute line like in [`start_selection`].
    pub fn extend_selection(&mut self, cx: usize, vy: usize) {
        if let Some(sel) = &mut self.selection {
            let total = self.buf.lines.len();
            let offset = total.saturating_sub(self.buf.rows);
            sel.end = (cx, offset.saturating_sub(self.view_offset) + vy);
        }
    }

    /// Drop the current selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Select the entire buffer — scrollback included, not just the
    /// visible viewport.
    pub fn select_all(&mut self) {
        let last_row = self.buf.lines.len().saturating_sub(1);
        let last_col = self.buf.cols.saturating_sub(1);
        self.selection = Some(Selection {
            start: (0, 0),
            end: (last_col, last_row),
        });
    }

    // =========================================================================
    // Scrollback viewport + search
    // =========================================================================

    /// Scroll the viewport by `delta` lines (negative = up / into
    /// scrollback, positive = down). Clamped to the buffer extent.
    pub fn scroll_lines(&mut self, delta: i32) {
        let total = self.buf.lines.len();
        let max_off = total.saturating_sub(self.buf.rows);
        let next = self.view_offset as i64 - delta as i64;
        self.view_offset = next.clamp(0, max_off as i64) as usize;
    }

    /// Snap the viewport back to the live bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.view_offset = 0;
    }

    /// (Re)run a case-insensitive search over the entire buffer
    /// (scrollback included). Empty query clears the search. The
    /// initially focused match is the last (most recent) one.
    pub fn search_set(&mut self, query: &str) {
        if query.is_empty() {
            self.search = None;
            return;
        }
        let needle_lower: Vec<char> = query
            .chars()
            .flat_map(|c| c.to_lowercase())
            .collect();
        let needle_chars = needle_lower.len();
        let mut matches = Vec::new();
        for (li, line) in self.buf.lines.iter().enumerate() {
            // Flatten the row: per-character text plus each char's visual
            // column span (start col, end col).
            let mut text_chars: Vec<char> = Vec::new();
            let mut col_start: Vec<usize> = Vec::new();
            let mut col_end: Vec<usize> = Vec::new();
            let mut col = 0usize;
            for cell in line.cells.iter() {
                if cell.width == 0 {
                    continue;
                }
                let w = cell.width as usize;
                for ch in cell.chars().chars() {
                    text_chars.push(ch);
                    col_start.push(col);
                    col_end.push(col + w);
                }
                col += w;
            }
            if text_chars.is_empty() {
                continue;
            }
            // Case-insensitive, CHAR-ALIGNED matching. Never byte-slice the
            // text: prompt glyphs like powerline separators (U+E0B0) are
            // multi-byte, and `str[start..end]` panics on non-boundaries.
            // Per-char `to_lowercase` keeps text/needle indexes aligned even
            // where full-string lowercasing would change char counts.
            let n = needle_chars;
            let mut start = 0usize;
            while start + n <= text_chars.len() {
                if text_chars[start..start + n]
                    .iter()
                    .zip(needle_lower.iter())
                    .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
                {
                    let cs = col_start[start];
                    let ce = col_end[start + n - 1];
                    matches.push(SearchMatch {
                        line: li,
                        col_start: cs,
                        col_end: ce,
                        text: text_chars[start..start + n].iter().collect(),
                    });
                }
                start += 1;
            }
        }
        let current = matches.len().saturating_sub(1);
        self.search = Some(Search {
            query: query.to_string(),
            matches,
            current,
        });
        self.search_reveal();
    }

    /// Move the focused match forward/backward (wrapping) and reveal it.
    /// Returns `(current, total)` (1-based current) for the UI counter.
    pub fn search_next(&mut self, forward: bool) -> Option<(usize, usize)> {
        let s = self.search.as_mut()?;
        if s.matches.is_empty() {
            return Some((0, 0));
        }
        s.current = if forward {
            (s.current + 1) % s.matches.len()
        } else {
            (s.current + s.matches.len() - 1) % s.matches.len()
        };
        let (cur, total) = (s.current, s.matches.len());
        self.search_reveal();
        Some((cur + 1, total))
    }

    /// The currently focused match, if any.
    pub fn search_current(&self) -> Option<&SearchMatch> {
        self.search.as_ref().and_then(|s| s.matches.get(s.current))
    }

    /// Adjust `view_offset` so the focused match is visible (a quarter of
    /// the way down the viewport).
    fn search_reveal(&mut self) {
        let Some(s) = &self.search else {
            return;
        };
        let Some(m) = s.matches.get(s.current) else {
            return;
        };
        let total = self.buf.lines.len();
        let rows = self.buf.rows;
        if total <= rows {
            self.view_offset = 0;
            return;
        }
        let base = total - rows;
        let want = m.line.saturating_sub(rows / 4).min(base);
        self.view_offset = base - want;
    }

    /// Extract the selected text. Cells are walked from the normalized
    /// start to end (inclusive of the end cell). Wide-char
    /// continuation cells (`width == 0`) are skipped. Each viewport row
    /// maps to an absolute line via `lines.len() - rows` (bottom-snap
    /// offset), which is correct while the terminal is snapped to the
    /// bottom — the common case.
    pub fn selected_text(&self) -> String {
        let Some(sel) = self.selection else {
            return String::new();
        };
        let (start, end) = normalize(sel);
        let total = self.buf.lines.len();
        let mut out = String::new();
        // Selection rows are absolute buffer line indices — copy exactly
        // that range regardless of where the viewport is right now.
        for line_idx in start.1..=end.1 {
            if line_idx >= total {
                break;
            }
            let line = &self.buf.lines[line_idx];
            let cell_start = if line_idx == start.1 { start.0 } else { 0 };
            let cell_end_exclusive = if line_idx == end.1 {
                (end.0 + 1).min(line.cells.len())
            } else {
                line.cells.len()
            };
            let mut row_text = String::new();
            for cell in line.cells.iter().take(cell_end_exclusive).skip(cell_start) {
                if cell.width == 0 {
                    continue;
                }
                row_text.push_str(&cell.chars());
            }
            out.push_str(row_text.trim_end());
            out.push('\n');
        }
        out.trim_end().to_string()
    }

    /// Whether the current selection spans more than a single cell
    /// (i.e. the user actually dragged, not just clicked).
    pub fn selection_is_range(&self) -> bool {
        let Some(sel) = self.selection else {
            return false;
        };
        let (s, e) = normalize(sel);
        s != e
    }
}

/// Return `(start, end)` with `start <= end` (row-major, then cell).
pub fn normalize(sel: Selection) -> ((usize, usize), (usize, usize)) {
    let a = sel.start;
    let b = sel.end;
    if a.1 < b.1 || (a.1 == b.1 && a.0 <= b.0) {
        (a, b)
    } else {
        (b, a)
    }
}

/// How the terminal cursor should be drawn this frame, mirroring
/// JediTerm's `TerminalCursorState` enum.
pub enum CursorRenderState {
    /// Focused & blink on-phase (or just moved): solid filled block.
    Showing,
    /// Not focused: hollow rectangle outline (still visible, no blink).
    NoFocus,
    /// DECSET ?25 off, or focused & blink off-phase: draw nothing.
    Hidden,
}

/// Ease-out-cubic interpolation: starts fast, decelerates to rest.
pub fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t;
    1.0 - u * u * u
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::parser::Parser;

    fn test_term() -> Terminal {
        Terminal {
            buf: Buffer::new(40, 4, 100),
            parser: Parser::new(),
            pty: None,
            title: String::new(),
            prev_cursor: (0, 0),
            cursor_anim_t: 1.0,
            cursor_anim_active: false,
            cursor_blink_visible: true,
            last_blink: std::time::Instant::now(),
            selection: None,
            pending_pty_resize: None,
            view_offset: 0,
            search: None,
        }
    }

    #[test]
    fn search_survives_multibyte_prompt_chars() {
        let mut t = test_term();
        // A prompt line with powerline separators (3-byte U+E0B0).
        t.buf.write_char(0xe0b0, 1, "");
        t.buf.write_char(b'D' as u32, 1, "");
        t.buf.write_char(b'e' as u32, 1, "");
        t.buf.write_char(b'b' as u32, 1, "");
        t.buf.write_char(0xe0b0, 1, "");
        // Regression: used to panic with "not a char boundary".
        t.search_set("Deb");
        let s = t.search.as_ref().expect("matches expected");
        assert_eq!(s.matches.len(), 1);
        assert_eq!(s.matches[0].text, "Deb");
        t.search_set("");
        assert!(t.search.is_none());
    }
}
