use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE_NAME: &str = "vibes.toml";

/// Commented template written when no config exists yet. Also used as the source of truth for defaults.
pub const DEFAULT_CONFIG_TOML: &str = include_str!("../vibes.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub network: NetworkConfig,
    pub controller: ControllerMapping,
    pub timing: TimingConfig,
    #[serde(default)]
    pub grid: GridConfig,
    pub groups: Vec<VibeGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    pub beyond_ip: String,      // Default: "127.0.0.1"
    pub beyond_port: u16,       // Default: 8000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMapping {
    pub midi_device_name: String, // Substring match, e.g. "Launchpad" or "RP2040"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_channel: Option<u8>, // 1-16; None = listen on all channels
    pub key_rhythm_main: u8,      // MIDI Note for Key 1
    pub key_rhythm_sub: u8,       // MIDI Note for Key 2
    pub key_flash: u8,            // MIDI Note for Key 3
    pub key_hold_latch: u8,       // MIDI Note for Key 4
    pub key_vibe_next_lock: u8,   // MIDI Note for Key 5
    pub key_blackout: u8,         // MIDI Note for Key 6
    pub cc_slider_morph: u8,      // MIDI CC number for Slider 1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingConfig {
    pub default_bpm: f64,               // Fallback BPM before taps are registered (e.g., 128.0)
    pub group_duration_secs: u64,       // Time before group enters ARMED state (e.g., 45)
    pub pause_beats_to_switch: f64,     // Beats of silence required to execute armed switch (e.g., 2.0)
    pub quick_tap_threshold_ms: u64,    // Under 150ms = quantized decay; over 150ms = manual gate hold
    pub default_decay_beats: f64,       // How many beats a quick tap sustains (e.g., 1.0 or 2.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridConfig {
    /// Number of columns in the BEYOND cue grid. Used to turn row/col into BEYOND's flat cue index.
    pub columns: u8,
}

impl Default for GridConfig {
    fn default() -> Self {
        Self { columns: 10 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct CueCell {
    pub page: u8, // 1-based Page index in BEYOND
    pub row: u8,  // 1-based Row index
    pub col: u8,  // 1-based Column index
}

impl CueCell {
    /// BEYOND addresses cues by page + flat 1-based cue index (1..=255).
    pub fn cue_index(&self, grid_columns: u8) -> u32 {
        (self.row as u32).saturating_sub(1) * grid_columns as u32 + self.col as u32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SelectionMode {
    RoundRobin,
    ShuffleBag, // Random without immediate repeat
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepModifier {
    None,
    CycleColorPalette(Vec<u8>),       // Cycles BEYOND Live Control color slider positions on each tap
    CyclePangoScript(Vec<String>),    // Runs custom PangoScript per tap (e.g., X-invert toggle)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FlashBehavior {
    TriggerSoloCue(CueCell),          // Mutes rhythm cue while held, fires high-energy cue
    OverlayPangoScript {              // Keeps rhythm cue alive, injects FX while held
        on_press: String,
        on_release: String,
    },
}

fn default_slider_min() -> f32 {
    0.0
}

fn default_slider_max() -> f32 {
    100.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VibeGroup {
    pub name: String,
    pub selection_mode: SelectionMode,
    pub rhythm_cues: Vec<CueCell>,
    pub step_modifier: StepModifier,
    pub flash_behavior: FlashBehavior,
    /// PangoScript command name (e.g. "Brightness") or a full OSC address (e.g. "/beyond/master/livecontrol/sizey")
    pub slider_pangoscript_target: String,
    #[serde(default = "default_slider_min")]
    pub slider_min: f32,
    #[serde(default = "default_slider_max")]
    pub slider_max: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        toml::from_str(DEFAULT_CONFIG_TOML).expect("bundled vibes.toml template must parse")
    }
}

impl AppConfig {
    /// Resolve which config file to use: `--config <path>`, then beside the executable, then the
    /// current directory. If none exists, the new file goes beside the executable.
    pub fn resolve_path(cli_override: Option<&Path>) -> PathBuf {
        if let Some(p) = cli_override {
            return p.to_path_buf();
        }
        let beside_exe = Self::exe_dir().map(|d| d.join(CONFIG_FILE_NAME));
        if let Some(p) = &beside_exe {
            if p.exists() {
                return p.clone();
            }
        }
        let cwd = PathBuf::from(CONFIG_FILE_NAME);
        if cwd.exists() {
            return cwd;
        }
        beside_exe.unwrap_or(cwd)
    }

    fn exe_dir() -> Option<PathBuf> {
        let mut exe = std::env::current_exe().ok()?;
        exe.pop();
        Some(exe)
    }

    /// Load (or create from the template) and validate. Returns the config, its path, and warnings.
    pub fn load_or_create(cli_override: Option<&Path>) -> Result<(Self, PathBuf, Vec<String>), String> {
        let mut path = Self::resolve_path(cli_override);

        if !path.exists() {
            if let Err(e) = fs::write(&path, DEFAULT_CONFIG_TOML) {
                // Exe folder may be read-only (e.g. Program Files); fall back to the current directory.
                let fallback = PathBuf::from(CONFIG_FILE_NAME);
                if cli_override.is_none() && path != fallback && fs::write(&fallback, DEFAULT_CONFIG_TOML).is_ok() {
                    path = fallback;
                } else {
                    return Err(format!("Could not create config file {}: {}", path.display(), e));
                }
            }
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Could not read config file {}: {}", path.display(), e))?;
        let config: AppConfig = toml::from_str(&content)
            .map_err(|e| format!("Config file {} has an error:\n{}", path.display(), e))?;
        let warnings = config
            .validate()
            .map_err(|errs| format!("Config file {} has problems:\n  - {}", path.display(), errs.join("\n  - ")))?;
        Ok((config, path, warnings))
    }

    /// Returns warnings on success, or the list of fatal errors.
    pub fn validate(&self) -> Result<Vec<String>, Vec<String>> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // Network
        if self.network.beyond_port == 0 {
            errors.push("network.beyond_port must be between 1 and 65535".into());
        }
        if (self.network.beyond_ip.as_str(), self.network.beyond_port.max(1))
            .to_socket_addrs()
            .map(|mut a| a.next().is_none())
            .unwrap_or(true)
        {
            errors.push(format!(
                "network.beyond_ip '{}' is not a valid address (e.g. 127.0.0.1 or 192.168.1.20)",
                self.network.beyond_ip
            ));
        }

        // Controller
        let c = &self.controller;
        if let Some(ch) = c.midi_channel {
            if !(1..=16).contains(&ch) {
                errors.push(format!("controller.midi_channel must be 1-16 (got {})", ch));
            }
        }
        let keys = [
            ("key_rhythm_main", c.key_rhythm_main),
            ("key_rhythm_sub", c.key_rhythm_sub),
            ("key_flash", c.key_flash),
            ("key_hold_latch", c.key_hold_latch),
            ("key_vibe_next_lock", c.key_vibe_next_lock),
            ("key_blackout", c.key_blackout),
        ];
        let mut seen: HashMap<u8, &str> = HashMap::new();
        for (name, note) in keys {
            if note > 127 {
                errors.push(format!("controller.{} must be a MIDI note 0-127 (got {})", name, note));
            }
            if let Some(other) = seen.insert(note, name) {
                errors.push(format!("controller.{} ({}) is the same note as controller.{}", name, note, other));
            }
        }
        if c.cc_slider_morph > 127 {
            errors.push(format!("controller.cc_slider_morph must be a MIDI CC 0-127 (got {})", c.cc_slider_morph));
        }

        // Timing
        let t = &self.timing;
        if !t.default_bpm.is_finite() || !(40.0..=300.0).contains(&t.default_bpm) {
            errors.push(format!("timing.default_bpm must be between 40 and 300 (got {})", t.default_bpm));
        }
        if t.group_duration_secs < 1 {
            errors.push("timing.group_duration_secs must be at least 1".into());
        }
        if !t.pause_beats_to_switch.is_finite() || !(0.25..=64.0).contains(&t.pause_beats_to_switch) {
            errors.push(format!(
                "timing.pause_beats_to_switch must be between 0.25 and 64 (got {})",
                t.pause_beats_to_switch
            ));
        }
        if !t.default_decay_beats.is_finite() || !(0.0625..=64.0).contains(&t.default_decay_beats) {
            errors.push(format!(
                "timing.default_decay_beats must be between 0.0625 and 64 (got {})",
                t.default_decay_beats
            ));
        }
        if t.quick_tap_threshold_ms > 2000 {
            errors.push(format!(
                "timing.quick_tap_threshold_ms must be 2000 or less (got {})",
                t.quick_tap_threshold_ms
            ));
        }

        // Grid
        if self.grid.columns == 0 {
            errors.push("grid.columns must be at least 1".into());
        }

        // Groups
        if self.groups.is_empty() {
            errors.push("at least one [[groups]] entry is required".into());
        }
        for (i, g) in self.groups.iter().enumerate() {
            let label = format!("group {} ('{}')", i + 1, g.name);
            if g.rhythm_cues.is_empty() {
                errors.push(format!("{}: rhythm_cues is empty", label));
            }
            let mut cells: Vec<(&str, CueCell)> = g.rhythm_cues.iter().map(|c| ("rhythm cue", *c)).collect();
            if let FlashBehavior::TriggerSoloCue(cell) = &g.flash_behavior {
                cells.push(("flash cue", *cell));
            }
            for (kind, cell) in cells {
                if cell.page == 0 || cell.row == 0 || cell.col == 0 {
                    errors.push(format!(
                        "{}: {} page/row/col are 1-based and cannot be 0 ({:?})",
                        label, kind, cell
                    ));
                } else if self.grid.columns > 0 {
                    if cell.col > self.grid.columns {
                        errors.push(format!(
                            "{}: {} col {} is larger than grid.columns ({})",
                            label, kind, cell.col, self.grid.columns
                        ));
                    }
                    if cell.cue_index(self.grid.columns) > 255 {
                        errors.push(format!("{}: {} {:?} is beyond BEYOND's 255 cues per page", label, kind, cell));
                    }
                }
            }
            let mut unique = g.rhythm_cues.clone();
            unique.sort_by_key(|c| (c.page, c.row, c.col));
            unique.dedup();
            if unique.len() != g.rhythm_cues.len() {
                warnings.push(format!("{}: rhythm_cues contains duplicate cells", label));
            }
            match &g.step_modifier {
                StepModifier::CycleColorPalette(p) if p.is_empty() => {
                    warnings.push(format!("{}: CycleColorPalette list is empty", label))
                }
                StepModifier::CyclePangoScript(s) if s.is_empty() => {
                    warnings.push(format!("{}: CyclePangoScript list is empty", label))
                }
                _ => {}
            }
            if let FlashBehavior::OverlayPangoScript { on_press, on_release } = &g.flash_behavior {
                if on_press.trim().is_empty() {
                    warnings.push(format!("{}: flash on_press script is empty", label));
                }
                if on_release.trim().is_empty() {
                    warnings.push(format!("{}: flash on_release script is empty", label));
                }
            }
            if g.slider_pangoscript_target.trim().is_empty() {
                warnings.push(format!("{}: slider_pangoscript_target is empty; slider will do nothing", label));
            }
            if !g.slider_min.is_finite() || !g.slider_max.is_finite() {
                errors.push(format!("{}: slider_min/slider_max must be numbers", label));
            }
        }

        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors)
        }
    }

    /// Update only the [controller] table, preserving comments and formatting in the rest of the file.
    pub fn save_controller(path: &Path, mapping: &ControllerMapping) -> Result<(), String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Could not read {}: {}", path.display(), e))?;
        let mut doc: toml_edit::DocumentMut = content
            .parse()
            .map_err(|e| format!("Config file {} has an error:\n{}", path.display(), e))?;

        let controller = doc
            .entry("controller")
            .or_insert(toml_edit::table())
            .as_table_mut()
            .ok_or_else(|| "[controller] in config is not a table".to_string())?;

        let set = |t: &mut toml_edit::Table, key: &str, v: toml_edit::Value| {
            match t.get_mut(key).and_then(|i| i.as_value_mut()) {
                Some(existing) => {
                    // Keep trailing comments attached to the value
                    let decor = existing.decor().clone();
                    *existing = v;
                    *existing.decor_mut() = decor;
                }
                None => {
                    t.insert(key, toml_edit::Item::Value(v));
                }
            }
        };

        set(controller, "midi_device_name", mapping.midi_device_name.as_str().into());
        match mapping.midi_channel {
            Some(ch) => set(controller, "midi_channel", (ch as i64).into()),
            None => {
                controller.remove("midi_channel");
            }
        }
        set(controller, "key_rhythm_main", (mapping.key_rhythm_main as i64).into());
        set(controller, "key_rhythm_sub", (mapping.key_rhythm_sub as i64).into());
        set(controller, "key_flash", (mapping.key_flash as i64).into());
        set(controller, "key_hold_latch", (mapping.key_hold_latch as i64).into());
        set(controller, "key_vibe_next_lock", (mapping.key_vibe_next_lock as i64).into());
        set(controller, "key_blackout", (mapping.key_blackout as i64).into());
        set(controller, "cc_slider_morph", (mapping.cc_slider_morph as i64).into());

        fs::write(path, doc.to_string()).map_err(|e| format!("Could not write {}: {}", path.display(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_template_parses_and_validates() {
        let cfg: AppConfig = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let warnings = cfg.validate().unwrap();
        assert!(warnings.is_empty(), "{:?}", warnings);
    }

    #[test]
    fn default_round_trips() {
        let cfg = AppConfig::default();
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: AppConfig = toml::from_str(&s).unwrap();
        assert_eq!(back.groups.len(), cfg.groups.len());
    }

    #[test]
    fn rejects_bad_values() {
        let mut cfg = AppConfig::default();
        cfg.timing.default_bpm = 0.0;
        cfg.timing.pause_beats_to_switch = -1.0;
        cfg.controller.key_flash = cfg.controller.key_rhythm_main;
        cfg.controller.cc_slider_morph = 200;
        cfg.groups.clear();
        let errs = cfg.validate().unwrap_err();
        assert!(errs.len() >= 5, "{:?}", errs);
    }

    #[test]
    fn cue_index_is_row_major_and_one_based() {
        let c = CueCell { page: 1, row: 1, col: 1 };
        assert_eq!(c.cue_index(10), 1);
        let c = CueCell { page: 1, row: 3, col: 2 };
        assert_eq!(c.cue_index(10), 22);
    }

    #[test]
    fn save_controller_preserves_comments() {
        let dir = std::env::temp_dir().join(format!("beyond-vibe-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vibes.toml");
        fs::write(&path, DEFAULT_CONFIG_TOML).unwrap();
        let mut mapping = AppConfig::default().controller;
        mapping.key_rhythm_main = 60;
        mapping.midi_device_name = "Test Pad".into();
        AppConfig::save_controller(&path, &mapping).unwrap();
        let out = fs::read_to_string(&path).unwrap();
        assert!(out.contains("# beyond-vibe configuration file"));
        let cfg: AppConfig = toml::from_str(&out).unwrap();
        assert_eq!(cfg.controller.key_rhythm_main, 60);
        assert_eq!(cfg.controller.midi_device_name, "Test Pad");
        let _ = fs::remove_dir_all(&dir);
    }
}
