// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Persistent settings, stored as simple `key = value` lines in
//! `~/.config/korterm/config.conf`. No serde — the format is
//! intentionally tiny and hand-rolled (unknown keys are ignored, missing
//! values fall back to defaults).

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Whether the bottom statusbar is shown.
    pub statusbar_visible: bool,
    /// Whether tabs are laid out in a right-hand sidebar (vs titlebar).
    pub tabs_vertical: bool,
    /// Sidebar width in logical pixels (vertical-tabs mode).
    pub sidebar_width: f32,
    /// Remembered shell preference ("bash"/"zsh"/…, empty = system default).
    pub shell: String,
    /// Top glow color, 0xRRGGBB (0 = theme default).
    pub glow_blue: u32,
    /// Bottom glow color, 0xRRGGBB (0 = theme default).
    pub glow_amber: u32,
    /// Glow intensity multipliers, per glow (1.0 = default).
    pub glow_intensity_top: f32,
    pub glow_intensity_bottom: f32,
    /// Shortcut overrides: `"copy"` → `"ctrl+shift+c"`.
    pub keybinds: Vec<(String, String)>,
}

impl Config {
    /// Look up a persisted shortcut override by action id.
    pub fn keybind_of(&self, id: &str) -> Option<String> {
        self.keybinds
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v.clone())
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            statusbar_visible: true,
            tabs_vertical: false,
            sidebar_width: 180.0,
            shell: String::new(),
            glow_blue: 0x4a8cff,
            glow_amber: 0xc99a5b,
            glow_intensity_top: 1.0,
            glow_intensity_bottom: 1.0,
            keybinds: Vec::new(),
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("korterm"))
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.conf"))
}

/// One-time migration from the pre-rename config dir.
///
/// The app was called `kortina-terminal` before v1.0; on first run under
/// the new name, carry the old `config.conf` (and the quick-terminal
/// first-run hint marker) into `~/.config/korterm/` so preferences
/// survive the rename. Non-destructive: the old directory is left in
/// place, and an existing new-style config is never overwritten.
pub fn migrate_legacy() {
    let Some(new_dir) = config_dir() else {
        return;
    };
    let Some(legacy_dir) = dirs::config_dir().map(|d| d.join("kortina-terminal")) else {
        return;
    };

    // The quick-terminal first-run hint marker migrates independently of
    // the config file: a fresh install (no old config.conf) can still
    // have a dismissed hint that must not be shown again.
    let old_marker = legacy_dir.join("quick-hotkey-hint-shown");
    let new_marker = new_dir.join("quick-hotkey-hint-shown");
    if old_marker.exists() && !new_marker.exists() && std::fs::create_dir_all(&new_dir).is_ok() {
        let _ = std::fs::copy(&old_marker, &new_marker);
    }

    let new_conf = new_dir.join("config.conf");
    let old_conf = legacy_dir.join("config.conf");
    if old_conf.exists() && !new_conf.exists() && std::fs::create_dir_all(&new_dir).is_ok() {
        let _ = std::fs::copy(&old_conf, &new_conf);
    }
}

/// Load the config; any missing/corrupt value keeps its default.
pub fn load() -> Config {
    let mut cfg = Config::default();
    let Some(path) = config_path() else {
        return cfg;
    };
    migrate_legacy();
    let Ok(text) = std::fs::read_to_string(path) else {
        return cfg;
    };
    for line in text.lines() {
        parse_line(&mut cfg, line);
    }
    cfg
}

/// Parse one `key = value` line into `cfg`. Unknown keys are ignored,
/// corrupt values keep their current (default) value.
fn parse_line(cfg: &mut Config, line: &str) {
    let Some((key, value)) = line.split_once('=') else {
        return;
    };
    let key = key.trim();
    let value = value.trim();
    match key {
            "statusbar_visible" => cfg.statusbar_visible = value == "true",
            "tabs_vertical" => cfg.tabs_vertical = value == "true",
            "sidebar_width" => {
                if let Ok(w) = value.parse::<f32>() {
                    cfg.sidebar_width = w.clamp(120.0, 600.0);
                }
            }
            "shell" => cfg.shell = value.to_string(),
            "glow_blue" => {
                if let Ok(v) = u32::from_str_radix(value.trim_start_matches("0x"), 16) {
                    cfg.glow_blue = v;
                }
            }
            "glow_amber" => {
                if let Ok(v) = u32::from_str_radix(value.trim_start_matches("0x"), 16) {
                    cfg.glow_amber = v;
                }
            }
            "glow_intensity_top" => {
                if let Ok(v) = value.parse::<f32>() {
                    cfg.glow_intensity_top = v.clamp(0.1, 2.5);
                }
            }
            "glow_intensity_bottom" => {
                if let Ok(v) = value.parse::<f32>() {
                    cfg.glow_intensity_bottom = v.clamp(0.1, 2.5);
                }
            }
            k if k.starts_with("keybind_") => {
                let id = k.trim_start_matches("keybind_").to_string();
                let v = value.to_string();
                if let Some(slot) = cfg.keybinds.iter_mut().find(|(key, _)| *key == id) {
                    slot.1 = v;
                } else {
                    cfg.keybinds.push((id, v));
                }
            }
            _ => {}
        }
}

/// Persist the config; failures (no config dir, read-only fs) are silent —
/// the app works fine without persistence.
pub fn save(cfg: &Config) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serialize(cfg));
}

/// Render the config as `key = value` lines (the on-disk format).
fn serialize(cfg: &Config) -> String {
    let mut text = format!(
        "statusbar_visible = {}\ntabs_vertical = {}\nsidebar_width = {:.1}\nshell = {}\nglow_blue = {:#06x}\nglow_amber = {:#06x}\nglow_intensity_top = {:.2}\nglow_intensity_bottom = {:.2}\n",
        cfg.statusbar_visible,
        cfg.tabs_vertical,
        cfg.sidebar_width,
        cfg.shell,
        cfg.glow_blue,
        cfg.glow_amber,
        cfg.glow_intensity_top,
        cfg.glow_intensity_bottom
    );
    for (id, combo) in &cfg.keybinds {
        text.push_str(&format!("keybind_{id} = {combo}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_overrides_defaults() {
        let mut cfg = Config::default();
        parse_line(&mut cfg, "statusbar_visible = false");
        parse_line(&mut cfg, "tabs_vertical = true");
        parse_line(&mut cfg, "sidebar_width = 240");
        parse_line(&mut cfg, "shell = zsh");
        parse_line(&mut cfg, "glow_blue = 0x112233");
        assert!(!cfg.statusbar_visible);
        assert!(cfg.tabs_vertical);
        assert_eq!(cfg.sidebar_width, 240.0);
        assert_eq!(cfg.shell, "zsh");
        assert_eq!(cfg.glow_blue, 0x112233);
    }

    #[test]
    fn sidebar_width_is_clamped() {
        let mut cfg = Config::default();
        parse_line(&mut cfg, "sidebar_width = 99999");
        assert_eq!(cfg.sidebar_width, 600.0);
        parse_line(&mut cfg, "sidebar_width = 1");
        assert_eq!(cfg.sidebar_width, 120.0);
    }

    #[test]
    fn corrupt_values_keep_defaults() {
        let def = Config::default();
        let mut cfg = Config::default();
        parse_line(&mut cfg, "sidebar_width = banana");
        parse_line(&mut cfg, "glow_blue = zz");
        assert_eq!(cfg.sidebar_width, def.sidebar_width);
        assert_eq!(cfg.glow_blue, def.glow_blue);
    }

    #[test]
    fn parseable_outliers_are_clamped_not_rejected() {
        let mut cfg = Config::default();
        parse_line(&mut cfg, "glow_intensity_top = -42");
        assert_eq!(cfg.glow_intensity_top, 0.1);
    }

    #[test]
    fn glow_intensity_is_clamped() {
        let mut cfg = Config::default();
        parse_line(&mut cfg, "glow_intensity_top = 99");
        parse_line(&mut cfg, "glow_intensity_bottom = 0");
        assert_eq!(cfg.glow_intensity_top, 2.5);
        assert_eq!(cfg.glow_intensity_bottom, 0.1);
    }

    #[test]
    fn keybinds_are_parsed_and_upserted() {
        let mut cfg = Config::default();
        parse_line(&mut cfg, "keybind_copy = ctrl+shift+c");
        parse_line(&mut cfg, "keybind_copy = ctrl+insert");
        assert_eq!(cfg.keybinds, vec![("copy".into(), "ctrl+insert".into())]);
    }

    #[test]
    fn unknown_and_malformed_lines_are_ignored() {
        let mut cfg = Config::default();
        parse_line(&mut cfg, "no_equals_sign");
        parse_line(&mut cfg, "future_option = 1");
        parse_line(&mut cfg, "");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn serialize_parse_roundtrip() {
        let mut cfg = Config::default();
        cfg.tabs_vertical = true;
        cfg.sidebar_width = 260.0;
        cfg.shell = "fish".into();
        cfg.glow_blue = 0x112233;
        cfg.glow_intensity_top = 1.4;
        cfg.keybinds.push(("paste".into(), "ctrl+shift+v".into()));

        let mut parsed = Config::default();
        for line in serialize(&cfg).lines() {
            parse_line(&mut parsed, line);
        }
        assert_eq!(parsed, cfg);
    }
}
