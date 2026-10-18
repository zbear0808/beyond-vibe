pub mod janitor;
pub mod shuffle_bag;
pub mod tap_tempo;

use crate::beyond_osc::BeyondClient;
use crate::config::{AppConfig, FlashBehavior, SelectionMode, StepModifier};
use crate::events::{EngineEvent, MidiInputMessage};
use janitor::Janitor;
use shuffle_bag::ShuffleBag;
use std::time::{Duration, Instant};
use tap_tempo::TapTempo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VibeTransitionState {
    Active { entered_at: Instant },
    Armed,  // Group timer expired; waiting for pause or flash release
    Locked, // User locked current vibe group
}

#[derive(Debug, Clone)]
pub struct EngineSnapshot {
    pub current_group_index: usize,
    pub current_group_name: String,
    pub total_groups: usize,
    pub transition_state: VibeTransitionState,
    pub bpm: f64,
    pub last_rhythm_time: Option<Instant>,
    pub is_blackout: bool,
    pub active_cues_count: usize,
}

pub struct Engine {
    config: AppConfig,
    client: BeyondClient,
    tap_tempo: TapTempo,
    janitor: Janitor,
    current_group_idx: usize,
    transition_state: VibeTransitionState,
    shuffle_bags: Vec<ShuffleBag>,
    round_robin_counters: Vec<usize>,
    step_modifier_counters: Vec<usize>,
    
    // Key press states & timings
    rhythm_main_pressed_at: Option<Instant>,
    last_rhythm_activity: Instant,
    auto_off_at: Option<Instant>,
    is_latched: bool,
    is_blackout: bool,
}

impl Engine {
    pub fn new(config: AppConfig, client: BeyondClient) -> Self {
        let default_bpm = config.timing.default_bpm;
        let bags: Vec<ShuffleBag> = config
            .groups
            .iter()
            .map(|g| ShuffleBag::new(g.rhythm_cues.len()))
            .collect();
        let rr_counters = vec![0; config.groups.len()];
        let step_counters = vec![0; config.groups.len()];

        Self {
            config,
            client,
            tap_tempo: TapTempo::new(default_bpm),
            janitor: Janitor::new(),
            current_group_idx: 0,
            transition_state: VibeTransitionState::Active {
                entered_at: Instant::now(),
            },
            shuffle_bags: bags,
            round_robin_counters: rr_counters,
            step_modifier_counters: step_counters,
            rhythm_main_pressed_at: None,
            last_rhythm_activity: Instant::now(),
            auto_off_at: None,
            is_latched: false,
            is_blackout: false,
        }
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        let current_group_name = self
            .config
            .groups
            .get(self.current_group_idx)
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        EngineSnapshot {
            current_group_index: self.current_group_idx,
            current_group_name,
            total_groups: self.config.groups.len(),
            transition_state: self.transition_state.clone(),
            bpm: self.tap_tempo.bpm(),
            last_rhythm_time: Some(self.last_rhythm_activity),
            is_blackout: self.is_blackout,
            active_cues_count: self.janitor.active_cues_count(),
        }
    }

    pub fn handle_event(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::Tick(now) => self.handle_tick(now),
            EngineEvent::MidiInput { timestamp, message } => self.handle_midi(timestamp, message),
            EngineEvent::ForceNextVibe => self.transition_to_next_group(true),
            EngineEvent::ToggleVibeLock => self.toggle_vibe_lock(),
            EngineEvent::BlackoutToggle => self.toggle_blackout(),
            EngineEvent::Quit => {
                self.janitor.blackout_all(&self.client);
            }
        }
    }

    fn handle_tick(&mut self, now: Instant) {
        // 1. Check auto-decay deadline for rhythm cue if not held and not latched
        if !self.is_latched {
            if let Some(deadline) = self.auto_off_at {
                if now >= deadline && self.rhythm_main_pressed_at.is_none() {
                    self.janitor.stop_all_rhythm(&self.client);
                    self.auto_off_at = None;
                }
            }
        }

        // 2. Check Vibe rotation timer
        match self.transition_state {
            VibeTransitionState::Active { entered_at } => {
                let duration = Duration::from_secs(self.config.timing.group_duration_secs);
                if now.duration_since(entered_at) >= duration {
                    self.transition_state = VibeTransitionState::Armed;
                }
            }
            VibeTransitionState::Armed => {
                // Check Phrase Gap Trigger: No rhythm keys held & silence >= pause_beats_to_switch * beat_duration
                let beat_dur = self.tap_tempo.beat_duration();
                let pause_threshold = beat_dur.mul_f64(self.config.timing.pause_beats_to_switch);

                if self.rhythm_main_pressed_at.is_none()
                    && now.duration_since(self.last_rhythm_activity) >= pause_threshold
                {
                    self.transition_to_next_group(false);
                }
            }
            VibeTransitionState::Locked => {
                // User locked; do not auto switch
            }
        }
    }

    fn handle_midi(&mut self, timestamp: Instant, message: MidiInputMessage) {
        let mapping = &self.config.controller;

        match message {
            MidiInputMessage::NoteOn { note, velocity, .. } if velocity > 0 => {
                if note == mapping.key_rhythm_main {
                    self.on_rhythm_press(timestamp, true);
                } else if note == mapping.key_rhythm_sub {
                    self.on_rhythm_press(timestamp, false);
                } else if note == mapping.key_flash {
                    self.on_flash_press();
                } else if note == mapping.key_hold_latch {
                    self.is_latched = !self.is_latched;
                    if !self.is_latched && self.rhythm_main_pressed_at.is_none() {
                        self.janitor.stop_all_rhythm(&self.client);
                    }
                } else if note == mapping.key_vibe_next_lock {
                    self.toggle_vibe_lock();
                } else if note == mapping.key_blackout {
                    self.toggle_blackout();
                }
            }
            MidiInputMessage::NoteOff { note, .. }
            | MidiInputMessage::NoteOn { note, velocity: 0, .. } => {
                if note == mapping.key_rhythm_main {
                    self.on_rhythm_release(timestamp);
                } else if note == mapping.key_flash {
                    self.on_flash_release();
                }
            }
            MidiInputMessage::ControlChange { controller, value, .. } => {
                if controller == mapping.cc_slider_morph {
                    let target = &self.config.groups[self.current_group_idx].slider_pangoscript_target;
                    self.client.set_master_slider(target, value);
                }
            }
            MidiInputMessage::NoteOn { .. } => {}
        }
    }

    fn on_rhythm_press(&mut self, timestamp: Instant, is_main: bool) {
        if is_main {
            self.tap_tempo.record_tap(timestamp);
            self.rhythm_main_pressed_at = Some(timestamp);
        }
        self.last_rhythm_activity = timestamp;

        // Choose cue from current group
        if let Some(group) = self.config.groups.get(self.current_group_idx) {
            let cue_count = group.rhythm_cues.len();
            if cue_count > 0 {
                let cue_idx = match group.selection_mode {
                    SelectionMode::RoundRobin => {
                        let idx = self.round_robin_counters[self.current_group_idx] % cue_count;
                        self.round_robin_counters[self.current_group_idx] = (idx + 1) % cue_count;
                        idx
                    }
                    SelectionMode::ShuffleBag => {
                        self.shuffle_bags[self.current_group_idx]
                            .pick_next()
                            .unwrap_or(0) % cue_count
                    }
                };

                let cue = group.rhythm_cues[cue_idx];
                self.janitor.trigger_exclusive_rhythm(cue, &self.client);

                // Step Modifier
                match &group.step_modifier {
                    StepModifier::None => {}
                    StepModifier::CycleColorPalette(palette) if !palette.is_empty() => {
                        let step_idx = self.step_modifier_counters[self.current_group_idx] % palette.len();
                        let color_id = palette[step_idx];
                        self.client.send_pangoscript(&format!("ColorPaletteIndex {}", color_id));
                        self.step_modifier_counters[self.current_group_idx] = (step_idx + 1) % palette.len();
                    }
                    StepModifier::CyclePangoScript(scripts) if !scripts.is_empty() => {
                        let step_idx = self.step_modifier_counters[self.current_group_idx] % scripts.len();
                        self.client.send_pangoscript(&scripts[step_idx]);
                        self.step_modifier_counters[self.current_group_idx] = (step_idx + 1) % scripts.len();
                    }
                    _ => {}
                }
            }
        }

        // Set initial auto-decay deadline
        let beat_dur = self.tap_tempo.beat_duration();
        let decay_dur = beat_dur.mul_f64(self.config.timing.default_decay_beats);
        self.auto_off_at = Some(timestamp + decay_dur);
    }

    fn on_rhythm_release(&mut self, timestamp: Instant) {
        if let Some(pressed_at) = self.rhythm_main_pressed_at.take() {
            let held_ms = timestamp.duration_since(pressed_at).as_millis() as u64;
            if held_ms > self.config.timing.quick_tap_threshold_ms {
                // Held longer than threshold (manual gate hold): cut immediately
                if !self.is_latched {
                    self.janitor.stop_all_rhythm(&self.client);
                    self.auto_off_at = None;
                }
            }
            // If held <= threshold (quick tap), leave auto_off_at to expire naturally or cut by next tap
        }
    }

    fn on_flash_press(&mut self) {
        if let Some(group) = self.config.groups.get(self.current_group_idx) {
            match &group.flash_behavior {
                FlashBehavior::TriggerSoloCue(cell) => {
                    self.janitor.trigger_flash_solo(*cell, &self.client);
                }
                FlashBehavior::OverlayPangoScript { on_press, on_release } => {
                    self.janitor.trigger_flash_overlay(on_press, on_release, &self.client);
                }
            }
        }
    }

    fn on_flash_release(&mut self) {
        if let Some(group) = self.config.groups.get(self.current_group_idx) {
            match &group.flash_behavior {
                FlashBehavior::TriggerSoloCue(_) => {
                    self.janitor.release_flash_solo(&self.client);
                }
                FlashBehavior::OverlayPangoScript { .. } => {
                    self.janitor.release_flash_overlay(&self.client);
                }
            }
        }

        // Flash Mask Trigger: if state was Armed, release of flash switches group seamlessly
        if self.transition_state == VibeTransitionState::Armed {
            self.transition_to_next_group(false);
        }
    }

    pub fn transition_to_next_group(&mut self, force: bool) {
        if self.config.groups.is_empty() {
            return;
        }

        if !force && self.transition_state == VibeTransitionState::Locked {
            return;
        }

        self.current_group_idx = (self.current_group_idx + 1) % self.config.groups.len();
        self.transition_state = VibeTransitionState::Active {
            entered_at: Instant::now(),
        };
        // Reset step modifier counter for new group
        if self.current_group_idx < self.step_modifier_counters.len() {
            self.step_modifier_counters[self.current_group_idx] = 0;
        }
    }

    pub fn toggle_vibe_lock(&mut self) {
        match self.transition_state {
            VibeTransitionState::Locked => {
                self.transition_state = VibeTransitionState::Active {
                    entered_at: Instant::now(),
                };
            }
            _ => {
                self.transition_state = VibeTransitionState::Locked;
            }
        }
    }

    pub fn toggle_blackout(&mut self) {
        self.is_blackout = !self.is_blackout;
        if self.is_blackout {
            self.janitor.blackout_all(&self.client);
        }
    }
}

