// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! PTY backend — spawns a child shell on a pseudo-terminal.
//!
//! Uses `portable-pty` (from the WezTerm project) for cross-platform
//! openpty / ConPTY support.  The [`PtySession`] owns the master end of
//! the PTY and provides async-friendly read / write methods.

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{self, Read, Write};
use std::sync::mpsc;

/// A live PTY session: master writer + child handle.
pub struct PtySession {
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send>,
    /// The master end of the PTY — kept alive so we can resize the
    /// grid (set winsize) after spawn.  Without this the child shell
    /// never receives SIGWINCH and keeps rendering at the initial size.
    pub master: Box<dyn MasterPty + Send>,
    /// Channel for delivering read bytes to the UI thread.
    pub rx: mpsc::Receiver<Vec<u8>>,
}

impl PtySession {
    /// Spawn the user's default shell (or `cmd` on Windows) at the given size.
    pub fn spawn(cols: u16, rows: u16) -> io::Result<Self> {
        Self::spawn_with(cols, rows, None, None)
    }

    /// Spawn a specific shell program (e.g. `bash`, `zsh`, `fish`).
    /// When `program` is `None`, falls back to the user's default shell.
    /// When `cwd` is `None`, the shell starts in the user's home directory.
    pub fn spawn_with(
        cols: u16,
        rows: u16,
        program: Option<&str>,
        cwd: Option<&str>,
    ) -> io::Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Choose a shell.  If a specific program is requested, use it;
        // otherwise let portable-pty pick the default ($SHELL on Unix,
        // cmd/PowerShell on Windows).
        let mut cmd = match program {
            Some(prog) => CommandBuilder::new(prog),
            None => CommandBuilder::new_default_prog(),
        };
        let cwd_path = cwd
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/")));
        cmd.cwd(cwd_path);
        cmd.env("TERM", "xterm-256color");

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let writer = pty_pair
            .master
            .take_writer()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let reader = pty_pair
            .master
            .try_clone_reader()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Spawn a reader thread that forwards bytes through a BOUNDED
        // channel: when the UI thread stalls (recompose, heavy frame),
        // `send` blocks and the kernel's PTY buffer absorbs the burst —
        // natural backpressure instead of unbounded memory growth.
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let mut reader = reader;
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        // Keep the master alive so we can resize the PTY later.
        let master = pty_pair.master;

        Ok(PtySession {
            writer,
            child,
            master,
            rx,
        })
    }

    /// Async version of [`spawn_with`] — spawns the shell on a blocking
    /// thread so the UI thread is never blocked.
    pub async fn spawn_with_async(
        cols: u16,
        rows: u16,
        program: Option<&str>,
        cwd: Option<&str>,
    ) -> io::Result<Self> {
        let program = program.map(|s| s.to_string());
        let cwd = cwd.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            Self::spawn_with(cols, rows, program.as_deref(), cwd.as_deref())
        })
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
    }

    /// Send raw bytes to the PTY (keyboard input → shell).
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Drain bytes that arrived since the last call. Per-call capped so
    /// an output flood (`cat huge.log`, `yes`) cannot turn a single UI
    /// pump into an unbounded parse; leftovers stay queued for the next
    /// pump tick.
    pub fn drain(&self) -> Vec<u8> {
        const MAX_DRAIN: usize = 1 << 20; // 1 MiB per pump tick
        let mut out = Vec::new();
        while out.len() < MAX_DRAIN {
            match self.rx.try_recv() {
                Ok(chunk) => out.extend(chunk),
                Err(_) => break,
            }
        }
        out
    }

    /// Resize the PTY grid.  This sets the kernel winsize on the master
    /// fd, which delivers SIGWINCH to the child shell so it redraws at
    /// the new dimensions.
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    /// Whether the child process is still alive.
    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}