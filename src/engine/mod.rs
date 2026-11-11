pub mod janitor;
pub mod shuffle_bag;
pub mod tap_tempo;

use crate::beyond_osc::BeyondClient;
use crate::config::{AppConfig, SelectionMode, StepModifier};
use crate::events::{EngineEvent, MidiInputMessage, MidiStatus};
use janitor::Janitor;
use rosc::OscType;
use shuffle_bag::ShuffleBag;
use std::time::{Duration, Instant};
use tap_tempo::TapTempo;

/// Holding Key 5 this long toggles the vibe lock; a shorter tap skips to the next group.
const VIBE_LOCK_HOLD: Duration = Duration::from_millis(600);
/// Minimum spacing between slider OSC messages (fader sweeps can send hundreds of CCs per second).
const SLIDER_INTERVAL: Duration = Duration::from_millis(20);
/// MIDI "All Sound Off" / "All Notes Off" controllers.
const CC_ALL_SOUND_OFF: u8 = 120;
const CC_ALL_NOTES_OFF: u8 = 123;

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
    pub is_blackout: bool,
    pub is_latched: bool,
    pub active_cues_count: usize,
    pub midi_status: MidiStatus,
    pub osc_target: String,
    pub osc_errors: u64,
    pub last_osc_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RhythmKey {
    Main,
    Sub,
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
    rhythm_sub_pressed_at: Option<Instant>,
    /// Which key started the currently playing rhythm cue
    rhythm_owner: Option<RhythmKey>,
    flash_held: bool,
    vibe_key_pressed_at: Option<Instant>,
    vibe_key_hold_fired: bool,
    last_rhythm_activity: Instant,
    auto_off_at: Option<Instant>,
    is_latched: bool,
    is_blackout: bool,

    // Slider
    last_slider_value: Option<u8>,
    pending_slider: Option<u8>,
    last_slider_sent_at: Option<Instant>,

    midi_status: MidiStatus,
}

impl Engine {
    pub fn new(config: AppConfig, client: BeyondClient, now: Instant) -> Self {
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
            transition_state: VibeTransitionState::Active { entered_at: now },
            shuffle_bags: bags,
            round_robin_counters: rr_counters,
            step_modifier_counters: step_counters,
            rhythm_main_pressed_at: None,
            rhythm_sub_pressed_at: None,
            rhythm_owner: None,
            flash_held: false,
            vibe_key_pressed_at: None,
            vibe_key_hold_fired: false,
            last_rhythm_activity: now,
            auto_off_at: None,
            is_latched: false,
            is_blackout: false,
            last_slider_value: None,
            pending_slider: None,
            last_slider_sent_at: None,
            midi_status: MidiStatus::Searching,
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
            is_blackout: self.is_blackout,
            is_latched: self.is_latched,
            active_cues_count: self.janitor.active_cues_count(),
            midi_status: self.midi_status.clone(),
            osc_target: self.client.target().to_string(),
            osc_errors: self.client.send_errors(),
            last_osc_error: self.client.last_error(),
        }
    }

    /// Clear any cues left running in BEYOND (e.g. after a crash) so the show starts from a known state.
    pub fn startup_clear(&mut self) {
        self.client.stop_all_now();
    }

    pub fn handle_event(&mut self, event: EngineEvent, now: Instant) {
        match event {
            EngineEvent::MidiInput { timestamp, message } => self.handle_midi(timestamp, message),
            EngineEvent::MidiStatus(status) => {
                if matches!(status, MidiStatus::Disconnected(_)) {
                    self.release_all_held(now);
                }
                self.midi_status = status;
            }
            EngineEvent::ForceNextVibe => self.transition_to_next_group(now, true),
            EngineEvent::ToggleVibeLock => self.toggle_vibe_lock(now),
            EngineEvent::BlackoutToggle => self.toggle_blackout(),
            EngineEvent::Quit => self.shutdown(),
        }
    }

    /// Stop everything we started in BEYOND. Called on quit and from the panic handler.
    pub fn shutdown(&mut self) {
        self.janitor.blackout_all(&self.client);
    }

    pub fn tick(&mut self, now: Instant) {
        // 1. Auto-decay of the rhythm cue, unless latched or its key is still held (gate)
        if !self.is_latched {
            if let Some(deadline) = self.auto_off_at {
                if now >= deadline && !self.owner_key_held() {
                    self.janitor.stop_all_rhythm(&self.client);
                    self.rhythm_owner = None;
                    self.auto_off_at = None;
                }
            }
        }

        // 2. Key 5 long-press = lock toggle
        if let Some(pressed_at) = self.vibe_key_pressed_at {
            if !self.vibe_key_hold_fired && now.saturating_duration_since(pressed_at) >= VIBE_LOCK_HOLD {
                self.vibe_key_hold_fired = true;
                self.toggle_vibe_lock(now);
            }
        }

        // 3. Rate-limited slider output
        if let Some(value) = self.pending_slider {
            let due = self
                .last_slider_sent_at
                .map_or(true, |t| now.saturating_duration_since(t) >= SLIDER_INTERVAL);
            if due && !self.is_blackout {
                self.send_slider(value);
                self.pending_slider = None;
                self.last_slider_sent_at = Some(now);
            }
        }

        // 4. Vibe rotation timer
        match self.transition_state {
            VibeTransitionState::Active { entered_at } => {
                let duration = Duration::from_secs(self.config.timing.group_duration_secs);
                if now.saturating_duration_since(entered_at) >= duration {
                    self.transition_state = VibeTransitionState::Armed;
                }
            }
            VibeTransitionState::Armed => {
                // Phrase Gap Trigger: no keys held & silence >= pause_beats_to_switch * beat_duration
                let beat_dur = self.tap_tempo.beat_duration();
                let pause_threshold = beat_dur.mul_f64(self.config.timing.pause_beats_to_switch);

                if !self.any_performance_key_held()
                    && now.saturating_duration_since(self.last_rhythm_activity) >= pause_threshold
                {
                    self.transition_to_next_group(now, false);
                }
            }
            VibeTransitionState::Locked => {
                // User locked; do not auto switch
            }
        }
    }

    fn any_rhythm_key_held(&self) -> bool {
        self.rhythm_main_pressed_at.is_some() || self.rhythm_sub_pressed_at.is_some()
    }

    fn any_performance_key_held(&self) -> bool {
        self.any_rhythm_key_held() || self.flash_held
    }

    fn owner_key_held(&self) -> bool {
        match self.rhythm_owner {
            Some(RhythmKey::Main) => self.rhythm_main_pressed_at.is_some(),
            Some(RhythmKey::Sub) => self.rhythm_sub_pressed_at.is_some(),
            None => false,
        }
    }

    fn handle_midi(&mut self, timestamp: Instant, message: MidiInputMessage) {
        if let Some(ch) = self.config.controller.midi_channel {
            if message.channel() + 1 != ch {
                return;
            }
        }
        let mapping = &self.config.controller;

        match message {
            MidiInputMessage::NoteOn { note, velocity, .. } if velocity > 0 => {
                if note == mapping.key_blackout {
                    self.toggle_blackout();
                } else if self.is_blackout {
                    // Blackout holds output off: ignore every other trigger
                } else if note == mapping.key_rhythm_main {
                    self.on_rhythm_press(timestamp, RhythmKey::Main);
                } else if note == mapping.key_rhythm_sub {
                    self.on_rhythm_press(timestamp, RhythmKey::Sub);
                } else if note == mapping.key_flash {
                    self.on_flash_press();
                } else if note == mapping.key_hold_latch {
                    self.is_latched = !self.is_latched;
                    if !self.is_latched && !self.owner_key_held() {
                        self.janitor.stop_all_rhythm(&self.client);
                        self.rhythm_owner = None;
                        self.auto_off_at = None;
                    }
                } else if note == mapping.key_vibe_next_lock {
                    self.vibe_key_pressed_at = Some(timestamp);
                    self.vibe_key_hold_fired = false;
                }
            }
            MidiInputMessage::NoteOff { note, .. } | MidiInputMessage::NoteOn { note, .. } => {
                if note == mapping.key_rhythm_main {
                    self.on_rhythm_release(timestamp, RhythmKey::Main);
                } else if note == mapping.key_rhythm_sub {
                    self.on_rhythm_release(timestamp, RhythmKey::Sub);
                } else if note == mapping.key_flash {
                    self.on_flash_release(timestamp);
                } else if note == mapping.key_vibe_next_lock {
                    // Short tap = next group (a long hold already toggled the lock in tick)
                    let was_pressed = self.vibe_key_pressed_at.take().is_some();
                    if was_pressed && !self.vibe_key_hold_fired && !self.is_blackout {
                        self.transition_to_next_group(timestamp, true);
                    }
                }
            }
            MidiInputMessage::ControlChange { controller, value, .. } => {
                if controller == CC_ALL_NOTES_OFF || controller == CC_ALL_SOUND_OFF {
                    self.release_all_held(timestamp);
                } else if controller == mapping.cc_slider_morph {
                    self.last_slider_value = Some(value);
                    self.pending_slider = Some(value);
                }
            }
        }
    }

    fn send_slider(&self, value: u8) {
        if let Some(g) = self.config.groups.get(self.current_group_idx) {
            self.client
                .set_slider(&g.slider_pangoscript_target, g.slider_min, g.slider_max, value);
        }
    }

    fn on_rhythm_press(&mut self, timestamp: Instant, key: RhythmKey) {
        match key {
            RhythmKey::Main => {
                self.tap_tempo.record_tap(timestamp);
                self.rhythm_main_pressed_at = Some(timestamp);
            }
            RhythmKey::Sub => self.rhythm_sub_pressed_at = Some(timestamp),
        }
        self.last_rhythm_activity = timestamp;

        let idx = self.current_group_idx;
        let Some(group) = self.config.groups.get(idx) else {
            return;
        };
        let cue_count = group.rhythm_cues.len();
        if cue_count == 0 {
            return;
        }

        let cue_idx = match group.selection_mode {
            SelectionMode::RoundRobin => {
                let i = self.round_robin_counters[idx] % cue_count;
                self.round_robin_counters[idx] = (i + 1) % cue_count;
                i
            }
            SelectionMode::ShuffleBag => self.shuffle_bags[idx].pick_next().unwrap_or(0) % cue_count,
        };

        let cue = group.rhythm_cues[cue_idx];
        self.janitor.trigger_exclusive_rhythm(cue, &self.client);
        self.rhythm_owner = Some(key);

        // Step Modifier
        match &group.step_modifier {
            StepModifier::None => {}
            StepModifier::CycleColorPalette(palette) if !palette.is_empty() => {
                let step_idx = self.step_modifier_counters[idx] % palette.len();
                self.client.send(
                    "/beyond/general/ColorSlider",
                    vec![OscType::Float(palette[step_idx] as f32)],
                );
                self.step_modifier_counters[idx] = (step_idx + 1) % palette.len();
            }
            StepModifier::CyclePangoScript(scripts) if !scripts.is_empty() => {
                let step_idx = self.step_modifier_counters[idx] % scripts.len();
                self.client.run_script(&scripts[step_idx]);
                self.step_modifier_counters[idx] = (step_idx + 1) % scripts.len();
            }
            _ => {}
        }

        // Set initial auto-decay deadline
        let beat_dur = self.tap_tempo.beat_duration();
        let decay_dur = beat_dur.mul_f64(self.config.timing.default_decay_beats);
        self.auto_off_at = timestamp.checked_add(decay_dur);
    }

    fn on_rhythm_release(&mut self, timestamp: Instant, key: RhythmKey) {
        let pressed = match key {
            RhythmKey::Main => self.rhythm_main_pressed_at.take(),
            RhythmKey::Sub => self.rhythm_sub_pressed_at.take(),
        };
        let Some(pressed_at) = pressed else {
            return;
        };
        self.last_rhythm_activity = timestamp;

        let held_ms = timestamp.saturating_duration_since(pressed_at).as_millis() as u64;
        // Held longer than threshold (manual gate hold): cut immediately, but only if this key's cue is playing
        if held_ms > self.config.timing.quick_tap_threshold_ms
            && !self.is_latched
            && self.rhythm_owner == Some(key)
        {
            self.janitor.stop_all_rhythm(&self.client);
            self.rhythm_owner = None;
            self.auto_off_at = None;
        }
        // Quick tap: leave auto_off_at to expire naturally or be cut by the next tap
    }

    fn on_flash_press(&mut self) {
        self.flash_held = true;
        if let Some(group) = self.config.groups.get(self.current_group_idx) {
            self.janitor.trigger_flash(&group.flash_behavior, &self.client);
        }
    }

    fn on_flash_release(&mut self, timestamp: Instant) {
        if !self.flash_held {
            return;
        }
        self.flash_held = false;
        self.janitor.release_flash(&self.client);

        // Flash Mask Trigger: if state was Armed, release of flash switches group seamlessly
        if self.transition_state == VibeTransitionState::Armed && !self.any_rhythm_key_held() {
            self.transition_to_next_group(timestamp, false);
        }
    }

    /// Treat every held key as released (device unplugged or All Notes Off received).
    fn release_all_held(&mut self, now: Instant) {
        if self.flash_held {
            self.flash_held = false;
            self.janitor.release_flash(&self.client);
        }
        if self.any_rhythm_key_held() {
            self.rhythm_main_pressed_at = None;
            self.rhythm_sub_pressed_at = None;
            self.last_rhythm_activity = now;
            if !self.is_latched {
                self.janitor.stop_all_rhythm(&self.client);
                self.rhythm_owner = None;
                self.auto_off_at = None;
            }
        }
        self.vibe_key_pressed_at = None;
    }

    fn transition_to_next_group(&mut self, now: Instant, force: bool) {
        if self.config.groups.is_empty() {
            return;
        }

        let was_locked = self.transition_state == VibeTransitionState::Locked;
        if !force && was_locked {
            return;
        }

        self.current_group_idx = (self.current_group_idx + 1) % self.config.groups.len();
        self.transition_state = if was_locked {
            VibeTransitionState::Locked
        } else {
            VibeTransitionState::Active { entered_at: now }
        };
        // Reset step modifier counter for new group
        if let Some(c) = self.step_modifier_counters.get_mut(self.current_group_idx) {
            *c = 0;
        }
        // The new group may map the slider to a different parameter: apply the fader's current position
        if self.last_slider_value.is_some() {
            self.pending_slider = self.last_slider_value;
        }
    }

    fn toggle_vibe_lock(&mut self, now: Instant) {
        self.transition_state = match self.transition_state {
            VibeTransitionState::Locked => VibeTransitionState::Active { entered_at: now },
            _ => VibeTransitionState::Locked,
        };
    }

    fn toggle_blackout(&mut self) {
        self.is_blackout = !self.is_blackout;
        if self.is_blackout {
            self.janitor.blackout_all(&self.client);
            self.flash_held = false;
            self.rhythm_main_pressed_at = None;
            self.rhythm_sub_pressed_at = None;
            self.rhythm_owner = None;
            self.vibe_key_pressed_at = None;
            self.auto_off_at = None;
            self.is_latched = false;
        } else if self.last_slider_value.is_some() {
            self.pending_slider = self.last_slider_value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CueCell, FlashBehavior};
    use std::net::UdpSocket;

    struct Harness {
        engine: Engine,
        rx: UdpSocket,
        t0: Instant,
    }

    impl Harness {
        fn new(mut cfg: AppConfig) -> Self {
            cfg.timing.group_duration_secs = 1;
            let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
            rx.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
            let port = rx.local_addr().unwrap().port();
            let client = BeyondClient::new("127.0.0.1", port, cfg.grid.columns).unwrap();
            let t0 = Instant::now();
            Self {
                engine: Engine::new(cfg, client, t0),
                rx,
                t0,
            }
        }

        fn at(&self, ms: u64) -> Instant {
            self.t0 + Duration::from_millis(ms)
        }

        fn note(&mut self, ms: u64, note: u8, on: bool) {
            let message = if on {
                MidiInputMessage::NoteOn { channel: 0, note, velocity: 100 }
            } else {
                MidiInputMessage::NoteOff { channel: 0, note }
            };
            let ts = self.at(ms);
            self.engine
                .handle_event(EngineEvent::MidiInput { timestamp: ts, message }, ts);
            self.engine.tick(ts);
        }

        fn tick(&mut self, ms: u64) {
            self.engine.tick(self.at(ms));
        }

        /// Drain received OSC messages as "addr args" strings
        fn sent(&self) -> Vec<String> {
            let mut out = Vec::new();
            let mut buf = [0u8; 1024];
            while let Ok(n) = self.rx.recv(&mut buf) {
                if let Ok((_, rosc::OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) {
                    out.push(format!("{} {:?}", m.addr, m.args));
                }
            }
            out
        }
    }

    fn cfg_with_flash(first: FlashBehavior, second: FlashBehavior) -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.groups[0].flash_behavior = first;
        cfg.groups[1].flash_behavior = second;
        cfg
    }

    #[test]
    fn blackout_blocks_triggers() {
        let mut h = Harness::new(AppConfig::default());
        let k = h.engine.config.controller.clone();
        h.note(0, k.key_blackout, true);
        h.sent();
        h.note(10, k.key_rhythm_main, true);
        h.note(20, k.key_flash, true);
        assert!(h.sent().is_empty());
        h.note(30, k.key_blackout, true);
        h.note(40, k.key_rhythm_main, true);
        assert!(h.sent().iter().any(|s| s.starts_with("/beyond/general/StartCue")));
    }

    #[test]
    fn solo_flash_released_after_group_change_to_overlay() {
        let solo = CueCell { page: 1, row: 2, col: 1 };
        let mut h = Harness::new(cfg_with_flash(
            FlashBehavior::TriggerSoloCue(solo),
            FlashBehavior::OverlayPangoScript { on_press: "ColorOn 1.0".into(), on_release: "ColorOn 0.0".into() },
        ));
        let k = h.engine.config.controller.clone();
        h.note(0, k.key_flash, true);
        h.engine.handle_event(EngineEvent::ForceNextVibe, h.at(10));
        h.sent();
        h.note(20, k.key_flash, false);
        let sent = h.sent();
        assert!(sent.iter().any(|s| s.starts_with("/beyond/general/StopCueNow [Int(1), Int(11)]")), "{:?}", sent);
        assert_eq!(h.engine.janitor.active_cues_count(), 0);
    }

    #[test]
    fn phrase_gap_waits_for_flash_release() {
        let mut h = Harness::new(AppConfig::default());
        let k = h.engine.config.controller.clone();
        h.tick(1_100);
        assert_eq!(h.engine.transition_state, VibeTransitionState::Armed);
        h.note(1_200, k.key_flash, true);
        h.tick(10_000);
        assert_eq!(h.engine.current_group_idx, 0);
        h.note(10_100, k.key_flash, false);
        assert_eq!(h.engine.current_group_idx, 1);
    }

    #[test]
    fn sub_key_hold_sustains_then_cuts_on_release() {
        let mut h = Harness::new(AppConfig::default());
        let k = h.engine.config.controller.clone();
        h.note(0, k.key_rhythm_sub, true);
        h.tick(2_000); // well past the 1-beat decay
        assert_eq!(h.engine.janitor.active_cues_count(), 1);
        h.note(2_100, k.key_rhythm_sub, false);
        assert_eq!(h.engine.janitor.active_cues_count(), 0);
    }

    #[test]
    fn releasing_main_does_not_cut_sub_cue() {
        let mut h = Harness::new(AppConfig::default());
        let k = h.engine.config.controller.clone();
        h.note(0, k.key_rhythm_main, true);
        h.note(1_000, k.key_rhythm_sub, true);
        h.note(1_050, k.key_rhythm_sub, false);
        h.note(1_100, k.key_rhythm_main, false);
        assert_eq!(h.engine.janitor.active_cues_count(), 1);
    }

    #[test]
    fn force_next_keeps_lock_and_vibe_key_tap_vs_hold() {
        let mut h = Harness::new(AppConfig::default());
        let k = h.engine.config.controller.clone();
        // Hold = lock
        h.note(0, k.key_vibe_next_lock, true);
        h.tick(700);
        h.note(800, k.key_vibe_next_lock, false);
        assert_eq!(h.engine.transition_state, VibeTransitionState::Locked);
        assert_eq!(h.engine.current_group_idx, 0);
        // Tap = next, stays locked
        h.note(1_000, k.key_vibe_next_lock, true);
        h.note(1_100, k.key_vibe_next_lock, false);
        assert_eq!(h.engine.current_group_idx, 1);
        assert_eq!(h.engine.transition_state, VibeTransitionState::Locked);
    }

    #[test]
    fn disconnect_releases_held_keys() {
        let mut h = Harness::new(AppConfig::default());
        let k = h.engine.config.controller.clone();
        h.note(0, k.key_rhythm_main, true);
        h.engine
            .handle_event(EngineEvent::MidiStatus(MidiStatus::Disconnected("pad".into())), h.at(500));
        assert_eq!(h.engine.janitor.active_cues_count(), 0);
        assert!(!h.engine.any_performance_key_held());
    }

    #[test]
    fn midi_channel_filter() {
        let mut cfg = AppConfig::default();
        cfg.controller.midi_channel = Some(2);
        let mut h = Harness::new(cfg);
        let k = h.engine.config.controller.clone();
        h.note(0, k.key_rhythm_main, true); // channel 1 -> ignored
        assert_eq!(h.engine.janitor.active_cues_count(), 0);
    }
}
