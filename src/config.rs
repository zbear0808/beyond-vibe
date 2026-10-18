use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub network: NetworkConfig,
    pub controller: ControllerMapping,
    pub timing: TimingConfig,
    pub groups: Vec<VibeGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub beyond_ip: String,      // Default: "127.0.0.1"
    pub beyond_port: u16,       // Default: 8000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerMapping {
    pub midi_device_name: String, // Substring match, e.g. "Launchpad" or "RP2040"
    pub key_rhythm_main: u8,      // MIDI Note for Key 1
    pub key_rhythm_sub: u8,       // MIDI Note for Key 2
    pub key_flash: u8,            // MIDI Note for Key 3
    pub key_hold_latch: u8,       // MIDI Note for Key 4
    pub key_vibe_next_lock: u8,   // MIDI Note for Key 5
    pub key_blackout: u8,         // MIDI Note for Key 6
    pub cc_slider_morph: u8,      // MIDI CC number for Slider 1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingConfig {
    pub default_bpm: f64,               // Fallback BPM before taps are registered (e.g., 128.0)
    pub group_duration_secs: u64,       // Time before group enters ARMED state (e.g., 45)
    pub pause_beats_to_switch: f64,     // Beats of silence required to execute armed switch (e.g., 2.0)
    pub quick_tap_threshold_ms: u64,    // Under 150ms = quantized decay; over 150ms = manual gate hold
    pub default_decay_beats: f64,       // How many beats a quick tap sustains (e.g., 1.0 or 2.0)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CueCell {
    pub page: u8, // 1-based Page index in BEYOND
    pub row: u8,  // 1-based Row index
    pub col: u8,  // 1-based Column index
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SelectionMode {
    RoundRobin,
    ShuffleBag, // Random without immediate repeat
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepModifier {
    None,
    CycleColorPalette(Vec<u8>),       // Cycles BEYOND Live Control Color indices on each tap
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VibeGroup {
    pub name: String,
    pub selection_mode: SelectionMode,
    pub rhythm_cues: Vec<CueCell>,
    pub step_modifier: StepModifier,
    pub flash_behavior: FlashBehavior,
    pub slider_pangoscript_target: String, // e.g., "SizeY" or "LaserOutput"
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            network: NetworkConfig {
                beyond_ip: "127.0.0.1".to_string(),
                beyond_port: 8000,
            },
            controller: ControllerMapping {
                midi_device_name: "".to_string(),
                key_rhythm_main: 36,
                key_rhythm_sub: 37,
                key_flash: 38,
                key_hold_latch: 39,
                key_vibe_next_lock: 40,
                key_blackout: 41,
                cc_slider_morph: 1,
            },
            timing: TimingConfig {
                default_bpm: 128.0,
                group_duration_secs: 45,
                pause_beats_to_switch: 2.0,
                quick_tap_threshold_ms: 150,
                default_decay_beats: 1.0,
            },
            groups: vec![
                VibeGroup {
                    name: "Atmosphere Waves".to_string(),
                    selection_mode: SelectionMode::ShuffleBag,
                    rhythm_cues: vec![
                        CueCell { page: 1, row: 1, col: 1 },
                        CueCell { page: 1, row: 1, col: 2 },
                        CueCell { page: 1, row: 1, col: 3 },
                        CueCell { page: 1, row: 1, col: 4 },
                    ],
                    step_modifier: StepModifier::CycleColorPalette(vec![1, 2, 3, 4]),
                    flash_behavior: FlashBehavior::TriggerSoloCue(CueCell { page: 1, row: 2, col: 1 }),
                    slider_pangoscript_target: "SizeY".to_string(),
                },
                VibeGroup {
                    name: "Peak Hour Beams".to_string(),
                    selection_mode: SelectionMode::ShuffleBag,
                    rhythm_cues: vec![
                        CueCell { page: 1, row: 3, col: 1 },
                        CueCell { page: 1, row: 3, col: 2 },
                        CueCell { page: 1, row: 3, col: 3 },
                    ],
                    step_modifier: StepModifier::None,
                    flash_behavior: FlashBehavior::OverlayPangoScript {
                        on_press: "Color 255,255,255".to_string(),
                        on_release: "ColorReset".to_string(),
                    },
                    slider_pangoscript_target: "LaserOutput".to_string(),
                },
            ],
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        // Look beside the executable first, then current working directory
        if let Ok(mut exe_path) = std::env::current_exe() {
            exe_path.pop();
            let beside_exe = exe_path.join("vibes.toml");
            if beside_exe.exists() {
                return beside_exe;
            }
        }
        PathBuf::from("vibes.toml")
    }

    pub fn load_or_create() -> Result<(Self, PathBuf), Box<dyn std::error::Error>> {
        let path = Self::config_path();
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            let config: AppConfig = toml::from_str(&content)?;
            Ok((config, path))
        } else {
            let default_cfg = AppConfig::default();
            let toml_str = toml::to_string_pretty(&default_cfg)?;
            fs::write(&path, toml_str)?;
            Ok((default_cfg, path))
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let toml_str = toml::to_string_pretty(self)?;
        fs::write(path, toml_str)?;
        Ok(())
    }
}

