// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! VT100/xterm escape-sequence parser.
//!
//! A byte-level state machine that splits the PTY output stream into
//! discrete actions: printable text, control characters (BS, CR, LF…),
//! CSI sequences (`ESC [ …`), OSC sequences (`ESC ] …`), and other ESC
//! commands.  Mirrors xterm.js `EscapeSequenceParser` but is much smaller
//! — it implements the subset needed by common shells (bash, zsh, fish).

/// A parsed output action for the [`crate::handler::InputHandler`] to apply.
#[derive(Clone, Debug)]
pub enum Action {
    /// Printable text (a single UTF-32 codepoint).
    Print { ch: char, width: u8, combined: String },
    /// C0 control character: 0x08 (BS), 0x09 (HT), 0x0A (LF), 0x0D (CR)…
    C0(u8),
    /// CSI sequence: `ESC [ params intermediate? final`.
    Csi { params: Vec<u16>, intermediates: Vec<u8>, final_byte: u8 },
    /// OSC sequence: `ESC ] data ST`.
    Osc { params: Vec<String>, data: String },
    /// Simple ESC sequence: `ESC <byte>`.
    Esc { intermediates: Vec<u8>, final_byte: u8 },
}

/// Parser state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Ground,
    Esc,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    StringCsi, // collecting OSC string data
    StringFin, // waiting for ST (ESC \ or BEL)
}

/// The escape-sequence parser.
#[derive(Clone, Debug)]
pub struct Parser {
    state: State,
    // CSI/ESC collection
    params: Vec<u16>,
    cur_param: Option<u16>,
    intermediates: Vec<u8>,
    final_byte: u8,
    // OSC collection
    osc_params: Vec<String>,
    osc_data: String,
    // UTF-8 decoding for printable text
    utf8: Vec<u8>,
    utf8_remaining: usize,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Parser {
            state: State::Ground,
            params: Vec::new(),
            cur_param: None,
            intermediates: Vec::new(),
            final_byte: 0,
            osc_params: Vec::new(),
            osc_data: String::new(),
            utf8: Vec::new(),
            utf8_remaining: 0,
        }
    }

    /// Feed a chunk of bytes from the PTY, returning the parsed actions.
    /// The caller should drain the returned `Vec` and apply each action.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Action> {
        let mut out = Vec::new();
        for &b in data {
            self.step(b, &mut out);
        }
        out
    }

    fn step(&mut self, b: u8, out: &mut Vec<Action>) {
        // --- UTF-8 multibyte accumulation in Ground state -------------------
        if self.state == State::Ground && self.utf8_remaining > 0 {
            self.utf8.push(b);
            self.utf8_remaining -= 1;
            if self.utf8_remaining == 0 {
                if let Some(s) = std::str::from_utf8(&self.utf8).ok() {
                    for ch in s.chars() {
                        let cp = ch as u32;
                        let w = char_width(cp);
                        out.push(Action::Print {
                            ch,
                            width: w,
                            combined: String::new(),
                        });
                    }
                }
                self.utf8.clear();
            }
            return;
        }

        match self.state {
            State::Ground => self.ground(b, out),
            State::Esc => self.esc(b, out),
            State::CsiEntry => self.csi_entry(b, out),
            State::CsiParam => self.csi_param(b, out),
            State::CsiIntermediate => self.csi_intermediate(b, out),
            State::StringCsi => self.string_csi(b, out),
            State::StringFin => self.string_fin(b, out),
        }
    }

    fn ground(&mut self, b: u8, out: &mut Vec<Action>) {
        // ESC enters escape mode.
        if b == 0x1B {
            self.state = State::Esc;
            self.intermediates.clear();
            self.params.clear();
            self.cur_param = None;
            return;
        }
        // C0 controls.
        if b < 0x20 || b == 0x7F {
            out.push(Action::C0(b));
            return;
        }
        // Printable / UTF-8.
        if b < 0x80 {
            // ASCII printable
            out.push(Action::Print {
                ch: b as char,
                width: 1,
                combined: String::new(),
            });
        } else {
            // Start of a UTF-8 multibyte sequence.
            let leading_ones = b.leading_ones();
            self.utf8_remaining = if leading_ones >= 2 {
                leading_ones as usize - 1
            } else {
                // Continuation byte without a leading byte — skip.
                return;
            };
            self.utf8.clear();
            self.utf8.push(b);
        }
    }

    fn esc(&mut self, b: u8, out: &mut Vec<Action>) {
        match b {
            b'[' => {
                self.state = State::CsiEntry;
                self.params.clear();
                self.cur_param = None;
                self.intermediates.clear();
            }
            b']' => {
                self.state = State::StringCsi;
                self.osc_params.clear();
                self.osc_data.clear();
            }
            // ESC followed by a final byte → simple ESC action.
            0x30..=0x7E => {
                out.push(Action::Esc {
                    intermediates: std::mem::take(&mut self.intermediates),
                    final_byte: b,
                });
                self.state = State::Ground;
            }
            // Intermediate bytes (0x20..0x2F) collected for ESC.
            0x20..=0x2F => {
                self.intermediates.push(b);
            }
            _ => {
                // C0 inside ESC — execute it and stay in ESC (simplified).
                if b < 0x20 {
                    out.push(Action::C0(b));
                } else {
                    self.state = State::Ground;
                }
            }
        }
    }

    fn csi_entry(&mut self, b: u8, out: &mut Vec<Action>) {
        // Private markers like '?' (0x3F) are stored as intermediates.
        match b {
            0x30..=0x39 => {
                // Digit — start collecting a parameter.
                self.cur_param = Some((b - b'0') as u16);
                self.state = State::CsiParam;
            }
            0x3B => {
                // ';' — parameter separator.
                self.params.push(0);
                self.state = State::CsiParam;
            }
            0x3F => {
                // '?' — private marker (DEC private mode).
                self.intermediates.push(b);
                self.state = State::CsiParam;
            }
            0x20..=0x2F => {
                self.intermediates.push(b);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                // Final byte with no parameters.
                self.final_byte = b;
                self.emit_csi(out);
                self.state = State::Ground;
            }
            _ => {
                // Unexpected — abort to ground.
                self.state = State::Ground;
            }
        }
    }

    fn csi_param(&mut self, b: u8, out: &mut Vec<Action>) {
        match b {
            0x30..=0x39 => {
                let digit = (b - b'0') as u16;
                let p = self.cur_param.get_or_insert(0);
                *p = (*p).saturating_mul(10).saturating_add(digit);
            }
            0x3B => {
                // ';' — push current param, start a new one.
                if let Some(p) = self.cur_param.take() {
                    self.params.push(p);
                } else {
                    self.params.push(0);
                }
            }
            0x3F => {
                // '?' — private marker appearing after a param
                // (unusual but legal).  Store as intermediate.
                if let Some(p) = self.cur_param.take() {
                    self.params.push(p);
                }
                self.intermediates.push(b);
                self.state = State::CsiIntermediate;
            }
            0x20..=0x2F => {
                // Intermediate byte.
                if let Some(p) = self.cur_param.take() {
                    self.params.push(p);
                }
                self.intermediates.push(b);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                // Final byte.
                if let Some(p) = self.cur_param.take() {
                    self.params.push(p);
                }
                self.final_byte = b;
                self.emit_csi(out);
                self.state = State::Ground;
            }
            _ => {
                self.state = State::Ground;
            }
        }
    }

    fn csi_intermediate(&mut self, b: u8, out: &mut Vec<Action>) {
        match b {
            0x20..=0x2F => self.intermediates.push(b),
            0x40..=0x7E => {
                self.final_byte = b;
                self.emit_csi(out);
                self.state = State::Ground;
            }
            _ => self.state = State::Ground,
        }
    }

    fn string_csi(&mut self, b: u8, out: &mut Vec<Action>) {
        // OSC data is collected until ST (ESC \) or BEL.
        match b {
            0x07 => {
                // BEL terminates OSC.
                self.emit_osc(out);
                self.state = State::Ground;
            }
            0x1B => {
                // ESC might be the start of ST (ESC \).
                self.state = State::StringFin;
            }
            _ => {
                // Split OSC into params at ';' on the first segment.
                self.osc_data.push(b as char);
            }
        }
    }

    fn string_fin(&mut self, b: u8, out: &mut Vec<Action>) {
        if b == b'\\' {
            // ST received — OSC complete.
            self.emit_osc(out);
            self.state = State::Ground;
        } else {
            // Not ST — treat the ESC as data and go back.
            self.osc_data.push(0x1B as char);
            self.state = State::StringCsi;
            // Re-process this byte in StringCsi.
            self.string_csi(b, out);
        }
    }

    fn emit_csi(&self, out: &mut Vec<Action>) {
        let mut params = self.params.clone();
        if let Some(p) = &self.cur_param {
            params.push(*p);
        }
        out.push(Action::Csi {
            params,
            intermediates: self.intermediates.clone(),
            final_byte: self.final_byte,
        });
    }

    fn emit_osc(&mut self, out: &mut Vec<Action>) {
        // Split the collected data into params at ';'.
        let data = std::mem::take(&mut self.osc_data);
        let params: Vec<String> = data.split(';').map(|s| s.to_string()).collect();
        if !params.is_empty() {
            // Last element is the "data" payload; the rest are the OSC
            // command number and sub-params.
            let data = params.last().cloned().unwrap_or_default();
            let cmd_params = params[..params.len().saturating_sub(1)].to_vec();
            out.push(Action::Osc {
                params: cmd_params,
                data,
            });
        }
    }
}

/// Approximate `wcwidth` for the codepoints a terminal is likely to see.
/// Returns 2 for CJK/wide chars, 1 for normal, 0 for combining marks.
fn char_width(cp: u32) -> u8 {
    if cp == 0 {
        return 0;
    }
    // Combining marks (U+0300..U+036F, etc.)
    if (0x0300..=0x036F).contains(&cp)
        || (0x1AB0..=0x1AFF).contains(&cp)
        || (0x1DC0..=0x1DFF).contains(&cp)
        || (0x20D0..=0x20FF).contains(&cp)
        || (0xFE20..=0xFE2F).contains(&cp)
    {
        return 0;
    }
    // CJK and other wide ranges.
    if (0x1100..=0x115F).contains(&cp)
        || (0x2E80..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7A3).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE30..=0xFE4F).contains(&cp)
        || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x20000..=0x2FFFD).contains(&cp)
        || (0x30000..=0x3FFFD).contains(&cp)
    {
        return 2;
    }
    1
}