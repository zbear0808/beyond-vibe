use crate::beyond_osc::BeyondClient;
use crate::config::{CueCell, FlashBehavior};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActiveFlash {
    Solo(CueCell),
    Overlay { release_script: String },
}

/// Tracks what we have started in BEYOND so it can always be stopped again.
#[derive(Default, Debug)]
pub struct Janitor {
    /// The single exclusive rhythm cue (logically active; muted in BEYOND while a solo flash plays)
    active_rhythm_cue: Option<CueCell>,
    active_flash: Option<ActiveFlash>,
}

impl Janitor {
    pub fn new() -> Self {
        Self::default()
    }

    fn solo_active(&self) -> bool {
        matches!(self.active_flash, Some(ActiveFlash::Solo(_)))
    }

    pub fn trigger_exclusive_rhythm(&mut self, new_cell: CueCell, client: &BeyondClient) {
        let solo = self.solo_active();
        if let Some(old) = self.active_rhythm_cue.take() {
            // While a solo flash plays, the old rhythm cue is already stopped in BEYOND
            if old != new_cell && !solo {
                client.stop_cue(old);
            }
        }
        if !solo {
            client.start_cue(new_cell);
        }
        // During a solo the new cue is remembered and started when the flash is released
        self.active_rhythm_cue = Some(new_cell);
    }

    pub fn stop_all_rhythm(&mut self, client: &BeyondClient) {
        if let Some(cell) = self.active_rhythm_cue.take() {
            if !self.solo_active() {
                client.stop_cue(cell);
            }
        }
    }

    pub fn trigger_flash(&mut self, behavior: &FlashBehavior, client: &BeyondClient) {
        // A second press without a release (lost NoteOff) must not orphan the first flash
        self.release_flash(client);

        match behavior {
            FlashBehavior::TriggerSoloCue(cell) => {
                if let Some(r) = self.active_rhythm_cue {
                    if r != *cell {
                        client.stop_cue(r);
                    }
                }
                client.start_cue(*cell);
                self.active_flash = Some(ActiveFlash::Solo(*cell));
            }
            FlashBehavior::OverlayPangoScript { on_press, on_release } => {
                client.run_script(on_press);
                self.active_flash = Some(ActiveFlash::Overlay {
                    release_script: on_release.clone(),
                });
            }
        }
    }

    /// Releases whatever flash is active, independent of the current group's config.
    pub fn release_flash(&mut self, client: &BeyondClient) {
        match self.active_flash.take() {
            Some(ActiveFlash::Solo(cell)) => {
                client.stop_cue(cell);
                if let Some(r) = self.active_rhythm_cue {
                    client.start_cue(r);
                }
            }
            Some(ActiveFlash::Overlay { release_script }) => {
                client.run_script(&release_script);
            }
            None => {}
        }
    }

    pub fn active_cues_count(&self) -> usize {
        let rhythm = usize::from(self.active_rhythm_cue.is_some() && !self.solo_active());
        rhythm + usize::from(self.active_flash.is_some())
    }

    /// Stops everything we know about, then tells BEYOND to stop and black out.
    pub fn blackout_all(&mut self, client: &BeyondClient) {
        match self.active_flash.take() {
            Some(ActiveFlash::Solo(cell)) => client.stop_cue(cell),
            Some(ActiveFlash::Overlay { release_script }) => client.run_script(&release_script),
            None => {}
        }
        if let Some(cell) = self.active_rhythm_cue.take() {
            client.stop_cue(cell);
        }
        client.stop_all_now();
        client.blackout();
    }
}
