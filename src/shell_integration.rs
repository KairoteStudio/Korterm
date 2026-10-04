//! Shell integration: syntax highlighting and tab completion for the
//! shells Korterm launches.
//!
//! A terminal emulator cannot highlight or complete anything by itself —
//! the *shell* owns the prompt. What Korterm can do is make the good
//! parts of each shell ecosystem available without the user hand-editing
//! their rc files:
//!
//! * **detect** which components are installed (highlighting,
//!   autosuggestions, completion),
//! * **install** a small, commented integration script and source it from
//!   the shell's rc file — idempotently, with a backup,
//! * **remove** it again, leaving no trace.
//!
//! The scripts themselves live in `shell-integration/*.sh` at the repo
//! root and are embedded into the binary with [`include_str!`], so they
//! stay readable and reviewable in the repository.

use std::io;
use std::path::{Path, PathBuf};

/// Opening line of the block Korterm adds to a user's rc file. Doubles
/// as the marker used to find (and remove) the block later.
pub const MARKER_BEGIN: &str = "# >>> korterm shell integration >>>";
/// Closing line of the managed rc block.
pub const MARKER_END: &str = "# <<< korterm shell integration <<<";

const ZSH_SCRIPT: &str = include_str!("../shell-integration/zsh.sh");
const BASH_SCRIPT: &str = include_str!("../shell-integration/bash.sh");
const SH_SCRIPT: &str = include_str!("../shell-integration/sh.sh");

/// Shell families Korterm knows how to enhance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Zsh,
    Bash,
    Sh,
    Fish,
    Other,
}

impl Family {
    /// Human-readable name, for the settings UI.
    pub fn label(self) -> &'static str {
        match self {
            Family::Zsh => "zsh",
            Family::Bash => "bash",
            Family::Sh => "sh",
            Family::Fish => "fish",
            Family::Other => "未识别的 Shell",
        }
    }

    /// `true` when Korterm can add anything for this shell. fish already
    /// ships highlighting and completion, so it needs no integration.
    pub fn is_supported(self) -> bool {
        matches!(self, Family::Zsh | Family::Bash | Family::Sh)
    }

    /// rc file sourced for interactive shells of this family.
    pub fn rc_path(self, home: &Path) -> PathBuf {
        match self {
            Family::Zsh => home.join(".zshrc"),
            Family::Bash => home.join(".bashrc"),
            Family::Sh => home.join(".profile"),
            Family::Fish => home.join(".config/fish/config.fish"),
            Family::Other => home.join(".profile"),
        }
    }

    /// Where Korterm stores the integration script for this family.
    pub fn script_path(self, home: &Path) -> PathBuf {
        home.join(".config")
            .join("korterm")
            .join("shell-integration")
            .join(match self {
                Family::Bash => "bash.sh",
                Family::Sh => "sh.sh",
                _ => "zsh.sh",
            })
    }

    /// Script body written to [`Family::script_path`].
    pub fn script(self) -> &'static str {
        match self {
            Family::Bash => BASH_SCRIPT,
            Family::Sh => SH_SCRIPT,
            _ => ZSH_SCRIPT,
        }
    }
}

/// Which enhancement component is present, and whether Korterm is
/// currently wiring it up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Support {
    /// Not found anywhere on this machine.
    Missing,
    /// Installed, but Korterm's integration is not enabled yet.
    Installed,
    /// Installed *and* sourced through Korterm's rc block.
    Enabled,
}

impl Support {
    pub fn label(self) -> &'static str {
        match self {
            Support::Missing => "未安装",
            Support::Installed => "已安装，未启用",
            Support::Enabled => "已启用",
        }
    }

    pub fn ok(self) -> bool {
        !matches!(self, Support::Missing)
    }
}

/// Snapshot of what this machine's shell setup offers.
#[derive(Clone, Debug)]
pub struct Status {
    pub family: Family,
    /// Shell as configured, for display ("/usr/bin/zsh").
    pub shell_display: String,
    /// Korterm's rc block is present.
    pub enabled: bool,
    /// Highlighting component (zsh plugin; N/A for others).
    pub syntax: Support,
    /// Autosuggestion component (zsh only).
    pub suggest: Support,
    /// Tab completion availability.
    pub completion: Support,
}

impl Status {
    /// apt command that installs everything still missing, or an empty
    /// string when nothing is missing.
    pub fn install_command(&self) -> String {
        let mut pkgs: Vec<&str> = Vec::new();
        match self.family {
            Family::Zsh => {
                if !self.syntax.ok() {
                    pkgs.push("zsh-syntax-highlighting");
                }
                if !self.suggest.ok() {
                    pkgs.push("zsh-autosuggestions");
                }
            }
            Family::Bash | Family::Sh if !self.completion.ok() => {
                pkgs.push("bash-completion");
            }
            _ => {}
        }
        if pkgs.is_empty() {
            String::new()
        } else {
            format!("sudo apt install {}", pkgs.join(" "))
        }
    }
}

/// Map a configured shell path to its family (`/usr/bin/zsh` → zsh).
pub fn family_of(shell: &str) -> Family {
    let base = shell
        .rsplit('/')
        .next()
        .unwrap_or(shell)
        .trim();
    match base {
        "zsh" => Family::Zsh,
        "bash" => Family::Bash,
        "sh" | "dash" | "ash" | "busybox-sh" => Family::Sh,
        "fish" => Family::Fish,
        _ => Family::Other,
    }
}

/// First existing path from `candidates`, or `None`.
fn first_existing(home: &Path, candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(|c| {
            if let Some(rest) = c.strip_prefix("~/") {
                home.join(rest)
            } else {
                PathBuf::from(c)
            }
        })
        .find(|p| p.exists())
}

fn zsh_syntax_candidates() -> &'static [&'static str] {
    &[
        "/usr/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh",
        "/usr/local/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh",
        "~/.local/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh",
        "~/.oh-my-zsh/custom/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh",
    ]
}

fn zsh_suggest_candidates() -> &'static [&'static str] {
    &[
        "/usr/share/zsh-autosuggestions/zsh-autosuggestions.zsh",
        "/usr/local/share/zsh-autosuggestions/zsh-autosuggestions.zsh",
        "~/.local/share/zsh-autosuggestions/zsh-autosuggestions.zsh",
        "~/.oh-my-zsh/custom/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh",
    ]
}

fn bash_completion_candidates() -> &'static [&'static str] {
    &[
        "/usr/share/bash-completion/bash_completion",
        "/etc/bash_completion",
        "/usr/local/share/bash-completion/bash_completion",
    ]
}

/// Detect the current state for `shell`, rooted at `home`.
pub fn status_in(shell: &str, home: &Path) -> Status {
    let family = family_of(shell);
    let enabled = has_block(&family.rc_path(home));

    let rc_text = std::fs::read_to_string(family.rc_path(home)).unwrap_or_default();
    // A component the user's own rc file already loads counts as active —
    // telling someone with working highlighting that it is "not enabled"
    // would be wrong.
    let active_in_rc = |needle: &str| rc_text.contains(needle);

    let (syntax, suggest, completion) = match family {
        Family::Zsh => {
            let syntax = if first_existing(home, zsh_syntax_candidates()).is_some() {
                if enabled || active_in_rc("zsh-syntax-highlighting") {
                    Support::Enabled
                } else {
                    Support::Installed
                }
            } else {
                Support::Missing
            };
            let suggest = if first_existing(home, zsh_suggest_candidates()).is_some() {
                if enabled || active_in_rc("zsh-autosuggestions") {
                    Support::Enabled
                } else {
                    Support::Installed
                }
            } else {
                Support::Missing
            };
            // compinit ships with zsh; it counts as active when the user's
            // rc already runs it (or our block does).
            let completion = if enabled || active_in_rc("compinit") {
                Support::Enabled
            } else {
                Support::Installed
            };
            (syntax, suggest, completion)
        }
        Family::Bash | Family::Sh => {
            let completion = if first_existing(home, bash_completion_candidates()).is_some() {
                if enabled || active_in_rc("bash_completion") || active_in_rc("bash-completion")
                {
                    Support::Enabled
                } else {
                    Support::Installed
                }
            } else {
                Support::Missing
            };
            (Support::Missing, Support::Missing, completion)
        }
        Family::Fish => (
            Support::Enabled,
            Support::Enabled,
            Support::Enabled,
        ),
        Family::Other => (
            Support::Missing,
            Support::Missing,
            Support::Missing,
        ),
    };

    Status {
        family,
        shell_display: shell.to_string(),
        enabled,
        syntax,
        suggest,
        completion,
    }
}

/// The rc block Korterm manages, ending with a newline.
pub fn rc_block(family: Family, home: &Path) -> String {
    let script = family.script_path(home);
    format!(
        "{MARKER_BEGIN}\n# 由 Korterm 设置 → Shell 生成；删除本段即可还原。\n\
         [ -r \"{script}\" ] && . \"{script}\"\n{MARKER_END}\n",
        script = script.display()
    )
}

/// `true` when the rc file already carries Korterm's block.
pub fn has_block(rc: &Path) -> bool {
    std::fs::read_to_string(rc)
        .map(|c| c.contains(MARKER_BEGIN) && c.contains(MARKER_END))
        .unwrap_or(false)
}

/// Remove every Korterm block from `text` (marker pairs included).
///
/// Used both for uninstalling and for re-writing the block in place, so
/// it must tolerate several blocks and unrelated content around them.
pub fn strip_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == MARKER_BEGIN {
            inside = true;
            continue;
        }
        if trimmed == MARKER_END {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Write the integration script and make sure the rc file sources it.
///
/// * the script is (re)written every time — it ships with the app;
/// * the rc block is inserted once, at the end (so it runs after the
///   user's own setup), and re-running replaces it in place;
/// * the rc file is backed up once, before the first edit.
pub fn enable_in(shell: &str, home: &Path) -> io::Result<()> {
    let family = family_of(shell);
    let script = family.script_path(home);
    if let Some(parent) = script.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&script, family.script())?;

    let rc = family.rc_path(home);
    let existing = std::fs::read_to_string(&rc).unwrap_or_default();
    let block = rc_block(family, home);
    if existing.contains(MARKER_BEGIN) && existing.contains(MARKER_END) {
        // Already wired up — refresh the block in place, leave the rest
        // of the rc file byte-identical.
        let refreshed = replace_blocks(&existing, &block);
        if refreshed != existing {
            std::fs::write(&rc, refreshed)?;
        }
        return Ok(());
    }

    // First edit: keep a one-time backup next to the rc file.
    if existing.is_empty() && !rc.exists() {
        // Nothing to back up; a fresh file is created below.
    } else {
        let backup = backup_path(&rc);
        if !backup.exists() {
            std::fs::write(&backup, &existing)?;
        }
    }

    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(&block);
    std::fs::write(&rc, content)?;
    Ok(())
}

/// Remove Korterm's block from the rc file (script file is left alone,
/// it is inert once nothing sources it).
pub fn disable_in(shell: &str, home: &Path) -> io::Result<()> {
    let family = family_of(shell);
    let rc = family.rc_path(home);
    let existing = match std::fs::read_to_string(&rc) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };
    let cleaned = strip_blocks(&existing);
    if cleaned != existing {
        std::fs::write(&rc, cleaned)?;
    }
    Ok(())
}

/// Replace all Korterm blocks in `text` with a single fresh `block`.
///
/// Idempotent: re-running on an already-managed file produces exactly
/// the same bytes, so repeated upgrades never accumulate blank lines.
fn replace_blocks(text: &str, block: &str) -> String {
    let stripped = strip_blocks(text);
    let head = stripped.trim_end_matches('\n');
    if head.trim().is_empty() {
        // Nothing but whitespace left — no separator needed.
        return block.to_string();
    }
    format!("{head}\n\n{block}")
}

/// Backup path Korterm writes before the first edit of an rc file:
/// `~/.zshrc` → `~/.zshrc.korterm.bak`.
pub fn backup_path(rc: &Path) -> PathBuf {
    let name = rc
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rc".into());
    rc.with_file_name(format!("{name}.korterm.bak"))
}

/// Status for the user's real home directory.
pub fn status(shell: &str) -> Status {
    status_in(shell, &home_dir())
}

/// Enable for the user's real home directory.
pub fn enable(shell: &str) -> io::Result<()> {
    enable_in(shell, &home_dir())
}

/// Disable for the user's real home directory.
pub fn disable(shell: &str) -> io::Result<()> {
    disable_in(shell, &home_dir())
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "korterm-shellint-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn family_detection_uses_the_basename() {
        assert_eq!(family_of("/usr/bin/zsh"), Family::Zsh);
        assert_eq!(family_of("zsh"), Family::Zsh);
        assert_eq!(family_of("/bin/bash"), Family::Bash);
        assert_eq!(family_of("/usr/bin/dash"), Family::Sh);
        assert_eq!(family_of("/usr/bin/fish"), Family::Fish);
        assert_eq!(family_of("/usr/bin/powershell"), Family::Other);
        assert_eq!(family_of(""), Family::Other);
    }

    #[test]
    fn enable_is_idempotent() {
        let home = tmp_home("idem");
        enable_in("/usr/bin/zsh", &home).unwrap();
        let rc = Family::Zsh.rc_path(&home);
        let first = std::fs::read_to_string(&rc).unwrap();
        assert!(first.contains(MARKER_BEGIN));

        enable_in("/usr/bin/zsh", &home).unwrap();
        let second = std::fs::read_to_string(&rc).unwrap();
        assert_eq!(first, second, "re-running must not touch the rc file");
        assert_eq!(second.matches(MARKER_BEGIN).count(), 1);
    }

    #[test]
    fn enable_preserves_user_content_and_keeps_a_backup() {
        let home = tmp_home("preserve");
        let rc = Family::Zsh.rc_path(&home);
        std::fs::write(&rc, "# my prompt\nexport FOO=bar\n").unwrap();

        enable_in("/usr/bin/zsh", &home).unwrap();
        let content = std::fs::read_to_string(&rc).unwrap();
        assert!(content.starts_with("# my prompt\nexport FOO=bar\n"));
        assert!(content.contains(MARKER_BEGIN));

        // Exactly one backup, holding the untouched original.
        let backup = backup_path(&rc);
        assert!(backup.exists(), "backup at {}", backup.display());
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "# my prompt\nexport FOO=bar\n"
        );
    }

    #[test]
    fn disable_removes_only_our_block() {
        let home = tmp_home("disable");
        let rc = Family::Zsh.rc_path(&home);
        std::fs::write(&rc, "before\n").unwrap();
        enable_in("/usr/bin/zsh", &home).unwrap();
        let with_after = format!("{}\nafter\n", std::fs::read_to_string(&rc).unwrap());
        std::fs::write(&rc, with_after).unwrap();

        disable_in("/usr/bin/zsh", &home).unwrap();
        let content = std::fs::read_to_string(&rc).unwrap();
        assert!(!content.contains(MARKER_BEGIN));
        assert!(!content.contains(MARKER_END));
        assert!(content.contains("before"));
        assert!(content.contains("after"));
    }

    #[test]
    fn block_points_at_the_installed_script() {
        let home = tmp_home("block");
        enable_in("/bin/bash", &home).unwrap();
        let rc = Family::Bash.rc_path(&home);
        let content = std::fs::read_to_string(&rc).unwrap();
        let script = Family::Bash.script_path(&home);
        assert!(content.contains(&script.display().to_string()));
        assert!(script.exists(), "script must be written");
    }

    #[test]
    fn re_enabling_after_a_path_change_updates_the_block() {
        // Simulates upgrading Korterm: same rc file, new script location.
        let home = tmp_home("update");
        let rc = Family::Zsh.rc_path(&home);
        std::fs::write(
            &rc,
            format!(
                "{}\n. /old/path/zsh.sh\n{}\nkeep me\n",
                MARKER_BEGIN, MARKER_END
            ),
        )
        .unwrap();

        enable_in("/usr/bin/zsh", &home).unwrap();
        let content = std::fs::read_to_string(&rc).unwrap();
        assert!(!content.contains("/old/path/zsh.sh"), "stale path remains");
        assert!(content.contains(&Family::Zsh.script_path(&home).display().to_string()));
        assert!(content.contains("keep me"));
        assert_eq!(content.matches(MARKER_BEGIN).count(), 1);
    }

    #[test]
    fn strip_handles_multiple_blocks_and_missing_end_marker() {
        let text = format!(
            "top\n{}\na\n{}\nmid\n{}\nb\n{}\nbottom\n",
            MARKER_BEGIN, MARKER_END, MARKER_BEGIN, MARKER_END
        );
        let cleaned = strip_blocks(&text);
        assert!(!cleaned.contains(MARKER_BEGIN));
        assert!(cleaned.contains("top"));
        assert!(cleaned.contains("mid"));
        assert!(cleaned.contains("bottom"));

        // Unterminated block: everything after the marker is dropped
        // rather than leaking managed text back into the rc file.
        let truncated = format!("top\n{}\nleftover\n", MARKER_BEGIN);
        assert_eq!(strip_blocks(&truncated), "top\n");
    }

    #[test]
    fn status_agrees_with_what_is_actually_installed() {
        // Deliberately environment-independent: what matters is that the
        // reported state matches the files on THIS machine, not a
        // hardcoded "nothing is installed".
        let home = tmp_home("status");
        let st = status_in("/usr/bin/zsh", &home);

        let syntax_present = first_existing(&home, zsh_syntax_candidates()).is_some();
        let suggest_present = first_existing(&home, zsh_suggest_candidates()).is_some();
        assert_eq!(
            st.syntax == Support::Missing,
            !syntax_present,
            "syntax state must match the presence of the plugin file"
        );
        assert_eq!(
            st.suggest == Support::Missing,
            !suggest_present,
            "autosuggest state must match the presence of the plugin file"
        );
        assert!(!st.enabled, "no block was written for this temp home");

        // The apt hint names exactly the pieces that are absent.
        let cmd = st.install_command();
        assert_eq!(cmd.contains("zsh-syntax-highlighting"), !syntax_present);
        assert_eq!(cmd.contains("zsh-autosuggestions"), !suggest_present);
        assert_eq!(cmd.is_empty(), syntax_present && suggest_present);
    }

    #[test]
    fn existing_user_setup_counts_as_active() {
        let home = tmp_home("preset");
        // User already runs compinit + the highlight plugin by hand.
        std::fs::write(
            Family::Zsh.rc_path(&home),
            "source /usr/share/zsh/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh\n\
             autoload -Uz compinit && compinit\n",
        )
        .unwrap();
        let st = status_in("/usr/bin/zsh", &home);
        assert!(!st.enabled, "Korterm's own block is absent");
        assert_eq!(st.completion, Support::Enabled);
    }

    #[test]
    fn install_command_is_empty_when_nothing_is_missing() {
        let home = tmp_home("complete");
        // Pretend the plugins exist so nothing is missing.
        let dir = home.join("fake-plugins");
        std::fs::create_dir_all(&dir).unwrap();
        let st = Status {
            family: Family::Zsh,
            shell_display: "zsh".into(),
            enabled: true,
            syntax: Support::Enabled,
            suggest: Support::Enabled,
            completion: Support::Enabled,
        };
        assert!(st.install_command().is_empty());
        let _ = dir;
    }

    #[test]
    fn fish_needs_no_integration() {
        let home = tmp_home("fish");
        let st = status_in("/usr/bin/fish", &home);
        assert!(!st.family.is_supported());
        assert_eq!(st.syntax, Support::Enabled);
        assert!(st.install_command().is_empty());
    }
}